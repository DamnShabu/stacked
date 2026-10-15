// The host side of the OpenSL ES translation: a PCM playback stream backed by
// PipeWire, reached without linking against it.
//
// This header carries no PipeWire types, deliberately. `opensles.cpp` builds
// the OpenSL object model against this instead of `<pipewire/pipewire.h>`
// directly, so the object model compiles the same way whether or not this
// tree was configured with pipewire-devel present — see the top of
// `pipewire_backend.cpp` for how that split is enforced.

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

namespace cordial::audio {

/// True once a PipeWire session has been confirmed reachable: the library
/// loaded, `pw_init` ran, and a core connection completed a round trip. The
/// first call does the work and every later call returns the cached result,
/// so this is cheap to call from `slCreateEngine` on every attempt.
///
/// False covers three different failures on purpose — headers absent at
/// build time, `libpipewire-0.3.so.0` absent at run time, and a reachable
/// library with no session behind it (`PIPEWIRE_RUNTIME_DIR` unset, daemon
/// not running) — because the caller's response is the same for all three:
/// report failure rather than hand back an engine with no audio behind it.
bool pipewire_available();

/// One PipeWire node that carries audio, as the registry describes it.
///
/// Everything here is copied verbatim out of the node's global properties
/// rather than interpreted: the caller that wants an
/// `android.media.AudioDeviceInfo.TYPE_*` out of it is `audio_classes.cpp`,
/// and Android's vocabulary belongs in the file that speaks Android. What
/// this file is responsible for is that each string is what PipeWire
/// actually said, so that a wrong guess upstream is a wrong guess about
/// honest data rather than a guess compounded on a guess.
struct DeviceInfo {
    /// The PipeWire global id, used unchanged as `AudioDeviceInfo.getId()`.
    /// Android only requires that ids be unique among the devices reported
    /// at the same moment and stable while a device stays connected, which
    /// is exactly what a PipeWire global id already is — so there is no
    /// mapping table here to drift out of step with the session.
    uint32_t id = 0;
    /// `node.name`: the stable routing target (`alsa_output.pci-...`), which
    /// is what a stream sets `PW_KEY_TARGET_OBJECT` to. Not shown to a user.
    std::string node_name;
    /// `node.description`: what the user's own volume control calls this
    /// device, and therefore what `getProductName()` must return if the
    /// device picker is to be recognisable.
    std::string description;
    /// `node.nick`, when set — the short form ("Speaker", "HDMI 1").
    std::string nick;
    /// `object.path` ("alsa:acp:sofhdadsp:3:playback", "bluez5:..."), whose
    /// prefix names the backing API. The most reliable single signal for the
    /// Android device type, because it comes from the SPA plugin that
    /// created the node rather than from a name someone can rename.
    std::string object_path;
    /// `media.class` was `Audio/Source` rather than `Audio/Sink`; i.e. this
    /// device records. Maps straight onto `AudioDeviceInfo.isSource()`.
    bool is_source = false;
    /// This node is the one the `default` metadata's `default.audio.sink` /
    /// `default.audio.source` key names — the device the user's desktop
    /// sends audio to when nothing has asked for anything else.
    bool is_default = false;
};

/// Every audio sink and source the session currently has, defaults marked.
///
/// **This opens no stream of any kind.** Listing a microphone is not using
/// one, and the whole privacy gate in `audio_classes.cpp` rests on that
/// distinction holding here: this walks the registry, reads properties, and
/// disconnects the registry listener again. A capture stream exists only
/// after `CaptureStream::open`, which nothing on the enumeration path calls.
///
/// Returns empty when no session is reachable, which the caller must report
/// as "no devices" rather than substituting a plausible-looking one.
std::vector<DeviceInfo> enumerate_devices();

/// The PipeWire sink Cordial has been asked to play into, as a stable
/// `node.name` — `alsa_output.pci-0000_00_1f.3-...`, not an index and not a
/// description.
///
/// Read once from `CORDIAL_AUDIO_SINK`, which the shell sets from the Audio
/// row in its settings. Empty is the ordinary state and means "follow
/// whatever the session calls the default sink", *and keeps following it* —
/// PipeWire moves a stream with no `PW_KEY_TARGET_OBJECT` when the default
/// changes, so the absence of a target is not a snapshot of today's default
/// but a standing instruction. Writing today's default in here instead would
/// pin the user to the speakers they happened to be using when they opened
/// settings.
///
/// **Indices and descriptions were both rejected as the stored form.** A
/// PipeWire global id renumbers when a device is unplugged and replugged, so
/// a stored index eventually names a different device; `node.description` is
/// localised and is what the user's own volume control renames when they
/// rename a device. `node.name` is the routing target `PW_KEY_TARGET_OBJECT`
/// takes and the only one of the three that is meant to be persisted.
///
/// One reader, here, on the same argument `aaudio.h` makes for `CORDIAL_AUDIO`:
/// each file calling `getenv` for itself is how a switch comes to mean two
/// different things in one process.
const std::string& configured_output_device();

/// The node name a playback stream should actually connect to, given what the
/// user asked for.
///
/// Empty in, empty out — no session is walked in that case, because "follow
/// the default" needs no lookup and paying two registry round trips per stream
/// open for the overwhelmingly common case would be a tax on everybody.
///
/// Non-empty in, this enumerates and checks the sink is *there*. **A device
/// that has gone away must not silently become silence**, which is what
/// setting `PW_KEY_TARGET_OBJECT` to an absent node produces: PipeWire has
/// nothing to link the stream to and the game plays into a node with no
/// output. So an absent sink falls back to the default and says so on stderr,
/// once per open, naming the sink that was asked for. AGENTS.md's rule against
/// a stub that lies is the same rule: reporting the gap keeps it findable, and
/// audio that quietly plays nowhere is the worst outcome available here.
std::string resolve_output_target(const std::string& requested);

/// How many `CaptureStream`s currently hold an open `pw_stream`.
///
/// Exists so the privacy requirement can be *checked* rather than asserted:
/// `audio_classes.cpp` logs this when the engine stops recording, and the
/// test binary reads it to prove enumeration left it at zero. `pw-top` and
/// the desktop's own microphone indicator are the independent checks; this
/// is the one Cordial can make about itself.
uint32_t active_capture_streams();

/// One playback stream, backed by one `pw_stream`. `opensles.cpp` owns one of
/// these per realized `AudioPlayer` object.
///
/// The enqueue/drain contract mirrors `SLAndroidSimpleBufferQueueItf`
/// directly rather than adding a layer of translation: `enqueue` takes
/// ownership of nothing and copies nothing — the caller's buffer must stay
/// valid and unmodified until the drain callback fires for it, exactly as
/// the Android buffer queue documents. That symmetry is what makes the
/// threading tractable: PipeWire's realtime side drains our internal queue
/// under a single mutex and calls back out to the caller with the mutex
/// already released, so a callback that re-enters `enqueue` (the ordinary
/// OpenSL pattern — queue the next buffer as soon as the last one drains)
/// does not deadlock against itself.
class PlaybackStream {
public:
    PlaybackStream();
    ~PlaybackStream();

    PlaybackStream(const PlaybackStream&) = delete;
    PlaybackStream& operator=(const PlaybackStream&) = delete;

    /// Creates and connects the underlying `pw_stream`, negotiating exactly
    /// the format described (no resampling surprises: what Roblox put in
    /// `SLDataFormat_PCM` is what reaches PipeWire's converter). `rate` is in
    /// Hz — `opensles.cpp` divides the spec's milliHertz value by 1000 before
    /// calling this, which is where that conversion belongs, not here.
    /// `container_bits` and `big_endian` pick the exact `spa_audio_format`;
    /// combinations this backend does not know how to describe return false
    /// rather than guessing at a nearby one.
    ///
    /// `target_node_name` aims the stream at one sink by `node.name`, the same
    /// parameter `CaptureStream::open` has always taken; empty connects to
    /// whatever PipeWire calls the default and keeps following it. It is
    /// passed through [`resolve_output_target`], so a sink that has been
    /// unplugged since the user chose it degrades to the default with a line
    /// in the log rather than to silence.
    bool open(uint32_t rate_hz, uint32_t channels, uint32_t bits_per_sample,
              uint32_t container_bits, bool big_endian, uint32_t max_pending_buffers,
              const std::string& target_node_name);

    void close();

    /// Queues `size` bytes of interleaved PCM starting at `data`. Never
    /// blocks: it either appends to the pending list under a short-held lock
    /// or, if the list is already at the player's declared buffer count,
    /// returns false immediately for the caller to report as
    /// `SL_RESULT_BUFFER_INSUFFICIENT`.
    bool enqueue(const void* data, uint32_t size, void* buffer_context);

    /// Discards every pending buffer without invoking the drain callback for
    /// any of them — matching `SLAndroidSimpleBufferQueueItf::Clear`, which
    /// is a discard, not a fast-forward.
    void clear();

    /// Called from PipeWire's own thread the moment a previously enqueued
    /// buffer has been fully copied out (i.e. "consumed", in the buffer
    /// queue's vocabulary). Registering a new callback replaces the old one;
    /// there is exactly one, matching
    /// `SLAndroidSimpleBufferQueueItf::RegisterCallback`.
    using DrainCallback = void (*)(void* buffer_context, void* user);
    void set_drain_callback(DrainCallback cb, void* user);

    void set_active(bool active);
    void set_volume_linear(float linear);
    void set_mute(bool mute);

    struct QueueState {
        uint32_t count; ///< buffers currently pending (mirrors `SLAndroidSimpleBufferQueueState.count`)
        uint32_t index; ///< buffers enqueued since the last `open`/`clear` (mirrors `.index`)
    };
    QueueState state() const;

private:
    struct Impl;
    Impl* impl_;
};
/// What the AAudio shim needs from a host audio backend, and nothing more.
///
/// **The seam exists so a second backend can be written without touching the
/// caller**, which ADR-023 decided and which the shape of this file made
/// necessary: `aaudio.cpp` used to hold a `CallbackStream` *by value* and call
/// it directly, so there was nowhere for PulseAudio or ALSA to go except in
/// place of PipeWire. The only polymorphism was a compile-time `#ifdef` with an
/// honest "no audio" arm — a two-way switch, not an n-way choice.
///
/// Every signature here is `CallbackStream`'s, unchanged. That is deliberate:
/// **a refactor that changes nothing is the one refactor that can be proved**,
/// and PipeWire is what every current user depends on. A second implementation
/// is a separate change, after this one is shown to have moved no number.
///
/// The realtime rule travels with the interface rather than with PipeWire:
/// whatever implements `FillCallback`'s caller must not lock, allocate, free,
/// log or make a syscall on that path. See `CallbackStream::set_running` for
/// the one method that is explicitly safe to call from inside it, and
/// `aaudio.cpp`'s header for the deadlock that rule was written after.
class OutputStream {
public:
    virtual ~OutputStream() = default;

    /// Fill `frames` frames of interleaved PCM at `dst`, in the negotiated
    /// format. Returns false to ask that the stream stop being pulled — the
    /// AAudio callback's `AAUDIO_CALLBACK_RESULT_STOP`.
    using FillCallback = bool (*)(void* dst, uint32_t frames, void* user);

    virtual bool open(uint32_t sample_bits, bool is_float, const char* node_description,
                      const char* target_node_name, FillCallback cb, void* user) = 0;
    virtual void close() = 0;
    virtual bool is_open() const = 0;
    virtual void set_running(bool running) = 0;
    virtual bool is_running() const = 0;
    virtual uint32_t rate_hz() const = 0;
    virtual uint32_t channels() const = 0;
    virtual uint32_t sample_bits() const = 0;
    virtual bool sample_is_float() const = 0;
    virtual uint32_t burst_frames() const = 0;
    virtual uint64_t silence_cycles() const = 0;
};

/// The host backend this run will use, named the way `CORDIAL_AUDIO_HOST`
/// spells it. Read once, from one place, and announced at startup -- which was written here before it was true and became true on 2026-08-28, after a user set the variable, got silence, and had nothing to read that would tell them whether it had been seen.
///
/// **A separate variable from `CORDIAL_AUDIO`, and ADR-023 says why.** That one
/// selects which *Android* API FMOD reaches Cordial through — AAudio, OpenSL,
/// or FMOD's Java path — and every combination of those with a host backend is
/// meaningful. One variable for two orthogonal axes is a variable nobody can
/// document.
const char* host_backend_name();

/// The backend this run will actually use, after probing when nobody named one.
///
/// `host_backend_name()` says what was asked for; this says what is there. See
/// the definition for the probe order and why PipeWire is still asked first.
const char* effective_backend_name();

/// Whether sound can come out on this host, by whichever backend was resolved.
///
/// **The predicate a one-way door must ask.** Asking `pipewire_available()`
/// instead is what made `CORDIAL_AUDIO_HOST` inert on the machines that needed
/// it: the door closed on PipeWire's absence before the selector ever ran.
bool host_backend_available();

/// Whether a PulseAudio server answered, proved by connecting to one.
///
/// As trustworthy as `pipewire_available()` has to be, and for the reason
/// ADR-023 records: `supportsAAudio()` is a one-way door, so a backend that
/// says yes and then cannot open leaves the session silent with no fallback.
bool pulse_available();

/// A PulseAudio stream, or null on a build without the headers.
///
/// Declared here rather than only in the factory's own file so the backend can
/// be exercised directly. **Waiting for the engine to open audio is not a
/// test**: measured on 2026-08-26, it opened on one healthy launch in six, so a
/// run with no sound proves nothing about the backend.
std::unique_ptr<OutputStream> make_pulse_stream();

/// Whether an ALSA PCM could be opened, proved by opening one.
///
/// Not free, unlike the other two probes -- it opens and closes a device -- and
/// cached for that reason. ADR-023 requires it anyway: `supportsAAudio()` is a
/// one-way door, so a backend that claims to work and then cannot open leaves
/// the whole session silent with no fallback.
bool alsa_available();

/// An ALSA stream, or null on a build without the headers.
std::unique_ptr<OutputStream> make_alsa_stream();

/// Whether `/dev/dsp` (or `CORDIAL_AUDIO_DEVICE`) can be opened for playback.
///
/// **The probe opens the device**, which on most OSS drivers is exclusive, so
/// asking briefly takes sound from whatever holds it. Cached, and only reached
/// when `CORDIAL_AUDIO_HOST=oss` names it -- see `oss_backend.cpp`.
bool oss_available();

/// An OSS output stream. `/dev/dsp` and three ioctls; no server, no library.
/// The one backend that works on a machine running no sound daemon at all.
std::unique_ptr<OutputStream> make_oss_stream();

/// A stream on whichever host backend this run selected.
///
/// Never null: a backend that cannot work returns an implementation whose
/// `open` fails, because a null here would make every call site test for it and
/// the failure it is reporting is one `open` already reports honestly.
std::unique_ptr<OutputStream> make_output_stream();


/// One playback stream that is *pulled* rather than pushed: PipeWire asks for
/// frames and this class asks its owner for them, in the same call, on the
/// same thread.
///
/// `PlaybackStream` above exists because `SLAndroidSimpleBufferQueueItf` is a
/// push interface — the engine hands over buffers and expects a drain
/// notification later, so something has to hold them in between, and that
/// something is a `std::deque` under a `std::mutex`. AAudio is not that shape.
/// `AAudioStreamBuilder_setDataCallback` installs a function AAudio calls on
/// its own realtime thread with "here is a buffer, fill it", which is
/// precisely PipeWire's `process()`. Bridging one to the other needs no
/// queue, no mutex, and no buffer of our own: `process()` calls the engine's
/// callback with PipeWire's own buffer and queues it.
///
/// **Nothing on this path may lock, allocate, free, log, or make a syscall**,
/// and the shape above is what makes that easy to hold to rather than a rule
/// to remember. The failure it forecloses is on record: `c7215eb` fixed an
/// AB-BA deadlock between `AudioDevice::close` holding its own mutex while
/// waiting for PipeWire's thread-loop lock and PipeWire's own thread holding
/// that lock while waiting for the mutex inside `AudioDevice::drained`. This
/// class has no mutex for the second half of that cycle to form against.
///
/// **The format is PipeWire's to choose, not ours to request.** The engine
/// build this was written for looks up no `AAudioStreamBuilder_setSampleRate`
/// and no `_setChannelCount` (see `docs/analysis/aaudio-contract.md`), so it
/// opens a stream and then reads back what it got. `open` therefore offers a
/// sample *format* and leaves rate and channel count unconstrained, waits for
/// PipeWire to negotiate them against whatever sink the session is already
/// running, and reports those. Nothing resamples on either side.
class CallbackStream : public OutputStream {
public:
    CallbackStream();
    ~CallbackStream() override;

    CallbackStream(const CallbackStream&) = delete;
    CallbackStream& operator=(const CallbackStream&) = delete;

    /// `sample_bits`/`is_float` describe the sample format to ask for; 0 bits
    /// means "no preference", which offers PipeWire's own float and so
    /// converts nothing. Blocks until PipeWire has both negotiated a format
    /// and run one cycle, so that `rate_hz`, `channels` and `burst_frames`
    /// are measured values by the time this returns true rather than
    /// placeholders the caller would go on to report as fact.
    ///
    /// `target_node_name` is the sink's `node.name`, or empty/null to follow
    /// the session default; see [`resolve_output_target`] for what happens
    /// when the named sink is not there.
    bool open(uint32_t sample_bits, bool is_float, const char* node_description,
              const char* target_node_name, FillCallback cb, void* user) override;

    void close() override;
    bool is_open() const override;

    /// Whether PipeWire is being asked for frames. False writes silence into
    /// every cycle rather than disconnecting, so this is safe to call from
    /// inside the fill callback — which `AAudioStream_requestStop` is
    /// documented to be reached from, and which taking PipeWire's loop lock
    /// here would deadlock against.
    void set_running(bool running) override;
    bool is_running() const override;

    uint32_t rate_hz() const override;
    uint32_t channels() const override;
    /// Bits per sample of the negotiated format, and whether it is float.
    uint32_t sample_bits() const override;
    bool sample_is_float() const override;
    /// Frames PipeWire asked for on the most recent cycle — the graph
    /// quantum, and the only truthful answer to `getFramesPerBurst`.
    uint32_t burst_frames() const override;

    /// Cycles that had to be filled with silence while the stream was
    /// running. See the note in `aaudio.cpp` on why this is not, and cannot
    /// be, Android's `getXRunCount` number.
    uint64_t silence_cycles() const override;

private:
    struct Impl;
    Impl* impl_;
};

/// Roblox's voice mute, set through `AppRtcDeviceWrapper.wrapSetCommunicationMute`
/// and honoured by `AAudioStream_read`, which zeroes what it hands back while
/// it is set.
///
/// A statement about content, not about the stream, so it does not bend the
/// rule below: a muted capture stream is still an open one, exactly as on
/// Android, where `AudioManager.setMicrophoneMute` silences the samples and
/// leaves the recording running. What it must not be is ignored -- a mute
/// button that keeps sending the room is the worst answer available.
inline std::atomic<bool>& voice_muted() {
    static std::atomic<bool> muted{false};
    return muted;
}

/// One capture stream, backed by one `pw_stream` in the input direction.
///
/// The lifetime rule is the point of this class and is not negotiable: an
/// instance holds no PipeWire resource at all until `open()`, and holds none
/// again the moment `close()` returns. There is no paused state and no muted
/// state, because neither of those puts the desktop's microphone indicator
/// out and neither of those stops another application seeing Cordial holding
/// the capture device. A recording that has stopped must be indistinguishable
/// from a Cordial that never recorded, and the only way to be
/// indistinguishable is to actually not be there.
///
/// That is why the read side is a ring buffer owned by this object rather
/// than a borrowed-pointer arrangement like `PlaybackStream`'s: blocking
/// Android readers and AAudio's callback-consumer thread both pull on their
/// own schedule, including not at all, and none of that may keep the stream
/// alive a moment longer than the engine asked for.
class CaptureStream {
public:
    CaptureStream();
    ~CaptureStream();

    CaptureStream(const CaptureStream&) = delete;
    CaptureStream& operator=(const CaptureStream&) = delete;

    /// Creates and connects a `pw_stream` capturing S16 PCM at `rate_hz` and
    /// `channels`, from `target_node_name` if it is non-empty and from
    /// whatever PipeWire calls the default source otherwise.
    ///
    /// **This is the only function in Cordial that opens the microphone.**
    /// Every caller of it must be on a path the engine explicitly asked to
    /// record on: `AudioRecord.startRecording`, `WebRtcAudioRecord.startRecording`,
    /// OpenSL's recorder transition, or `AAudioStream_requestStart`.
    bool open(uint32_t rate_hz, uint32_t channels, const std::string& target_node_name);

    /// Destroys the underlying `pw_stream` and drops every buffered sample.
    /// Idempotent, so the double-stop that a `stop()` followed by a
    /// `release()` produces costs nothing and is not an error.
    void close();

    bool is_open() const;

    /// True after PipeWire reports that this capture stream entered its error
    /// state. Cleared by the next successful `open`, so AAudio can translate
    /// a live backend failure into its DISCONNECTED state and error callback.
    bool failed() const;

    /// Copies up to `size` bytes of captured interleaved S16 PCM into `dst`,
    /// returning how many bytes were written. Returns 0 rather than blocking
    /// when the stream is closed or nothing has arrived yet: a recorder that
    /// blocks forever on a device that will never produce samples is the
    /// failure mode this whole file exists to avoid.
    uint32_t read(void* dst, uint32_t size);

    /// Bytes the ring had to drop because the reader was too slow, since
    /// `open`. This is the capture side's *real* overrun count, and unlike
    /// the playback side's it is one Cordial can see for itself: the ring is
    /// ours, the reader is ours, and a reader that falls half a second behind
    /// loses samples here rather than at the server.
    ///
    /// `AAudioStream_getXRunCount` on an input stream reports this, converted
    /// to frames. See the note beside that function in `aaudio.cpp` for why
    /// the *output* side has nothing equivalent to offer and returns a number
    /// that is structurally always zero.
    uint64_t dropped_bytes() const;

private:
    struct Impl;
    Impl* impl_;
};

/// Exposed only so `pipewire_backend_test.cpp` can check the underrun
/// behaviour without a live PipeWire session, hardware, or any risk of
/// producing sound: a periodic replay of stale buffer contents is exactly
/// what turns a silent gap into an audible tone, so the "pad with zeroed
/// silence rather than whatever the buffer held last cycle" rule is worth
/// checking on its own, not only by ear.
namespace testing {

/// One connection to something that may not be up yet, held for the process and
/// retried after a failure.
///
/// **A failure used to be permanent.** `get_session` was a function-local
/// static, so a PipeWire that was still starting when the first sound was
/// wanted (a login where the audio daemon comes up a few seconds after the
/// client) left the whole run without audio even though the daemon answered a
/// moment later. A success is kept for the life of the process, as before; a
/// failure is only remembered for `retry_after`, and the next caller after that
/// tries again. The wait keeps a wedged daemon, which costs three seconds per
/// attempt, from being asked on every call.
///
/// The connector and the clock are passed in so the retry rule can be checked
/// without a PipeWire session.
template <class T>
class RetryingSlot {
public:
    using Clock = std::chrono::steady_clock;

    explicit RetryingSlot(Clock::duration retry_after) : retry_after_(retry_after) {}

    /// `connect(first_attempt)` returns the connection or null. `now()` is read
    /// after it returns, so the wait is counted from when a failure ended and
    /// not from when it began.
    template <class Connect, class Now>
    T* get(Connect&& connect, Now&& now) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (value_) return value_;
        if (attempts_ > 0 && now() - last_attempt_ < retry_after_) return nullptr;
        const bool first = attempts_ == 0;
        ++attempts_;
        value_ = connect(first);
        last_attempt_ = now();
        return value_;
    }

    int attempts() const {
        std::lock_guard<std::mutex> lock(mutex_);
        return attempts_;
    }

private:
    mutable std::mutex mutex_;
    Clock::duration retry_after_;
    Clock::time_point last_attempt_{};
    T* value_ = nullptr;
    int attempts_ = 0;
};

struct PendingBuffer {
    const uint8_t* data;
    uint32_t size;
    uint32_t offset;
    void* context;
};

/// The matching half of [`resolve_output_target`], with the registry walk
/// taken out: given what the user asked for and the sink names a session
/// actually has, which target should the stream connect to?
///
/// Separated purely so it can be checked without a PipeWire session, because
/// the two answers that matter — "the chosen sink is still there" and "it has
/// gone, fall back to the default and say so" — are exactly the ones that are
/// awkward to produce on a live machine on demand. `*fell_back` is set true
/// only in the second case; a request that was empty to begin with has not
/// fallen back from anything and must not be reported as though a device had
/// disappeared.
std::string choose_output_target(const std::string& requested,
                                 const std::vector<std::string>& available_sinks,
                                 bool* fell_back);

/// Copies from the front of `pending` into `dst[0, want)`, popping buffers
/// as they empty (and recording their `context` in `drained_contexts`, in
/// order) and zero-filling any shortfall. Returns the number of trailing
/// bytes that had to be silence-padded — zero on a clean fill. This is
/// exactly what `PlaybackStream::Impl::process()` does each PipeWire
/// cycle, factored out because that method also touches real `pw_buffer`
/// and `pw_stream` objects that only exist with a live stream connected.
uint32_t fill_pcm(std::deque<PendingBuffer>& pending, uint8_t* dst, uint32_t want,
                   std::vector<void*>& drained_contexts);

} // namespace testing

} // namespace cordial::audio

// ------------------------------------------------------- the picker's window
//
// The shell's Audio row needs the same list the engine's device enumeration
// gets, and the shell is a separate process that links none of this tree's
// C++ — so the list has to cross a C ABI. Declared here rather than in a
// header of its own because it is the same data `enumerate_devices` returns,
// filtered to sinks, and a second header would invite a second answer.
//
// **Sinks only.** A device picker for *output* has no business listing
// microphones, and the microphone rule at the top of `audio_classes.cpp`
// makes the stronger point: nothing on this path may construct a
// `CaptureStream`, and the cheapest way to be sure of that is for the sources
// never to leave this function.

extern "C" {

/// One sink, as the shell shows it. Both pointers are NUL-terminated UTF-8
/// owned by the array they came in, and are valid until it is freed.
struct CordialAudioSink {
    /// `node.name` — what gets stored in `shell.json` and handed back as
    /// `CORDIAL_AUDIO_SINK`. Stable across replug; never shown to a user.
    const char* node_name;
    /// `node.description`, or `node.name` when the session gave no
    /// description. What the row displays.
    const char* description;
    /// Non-zero if this is the session's current `default.audio.sink`. The
    /// row marks it, so that "System default" and the device it currently
    /// resolves to are both visible at once.
    int is_default;
};

/// Fills `*out` with a freshly allocated array of every audio sink the session
/// has, and returns how many. Returns 0 and leaves `*out` null when there is
/// no session — which the caller must present as "no devices found", never as
/// an invented default, for the reason the no-PipeWire build's
/// `enumerate_devices` gives.
///
/// The caller owns the array and must hand it back to
/// `cordial_audio_sinks_free`.
size_t cordial_audio_sinks(CordialAudioSink** out);

void cordial_audio_sinks_free(CordialAudioSink* sinks, size_t count);

} // extern "C"
