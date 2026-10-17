// ALSA, the third host backend (ADR-023), and the one whose shape differs most.
//
// PipeWire and PulseAudio both hand this process a thread and ask it to fill a
// buffer, which is the pull contract `OutputStream` was drawn around. **ALSA
// hands over nothing.** There is no callback, no loop thread, and no "there is
// room for N" notification -- there is a device you write to, which blocks when
// it is full. So this backend owns a writer thread, and that thread is the only
// thing in the file that touches the device once it is open.
//
// That single difference is where a port like this goes wrong, and it goes
// wrong in three specific ways this file is arranged to avoid:
//
//   * **Reporting what was asked for rather than what was granted.** ALSA's
//     `_near` setters take a value and *change it* to the nearest the hardware
//     will do. The engine looks up no `AAudioStreamBuilder_setSampleRate` and
//     no `_setChannelCount` (`docs/analysis/aaudio-contract.md`); it opens a
//     stream and reads back what it got. So every number this reports is read
//     out of the configured device afterwards, never remembered from the
//     request.
//   * **Guessing the burst.** `getFramesPerBurst` is what FMOD multiplies by
//     nine to size itself, so it must be the real period size and nothing else.
//   * **Locking against the writer.** The obvious ALSA idiom is a mutex around
//     `snd_pcm_t*` taken by both the writer and `close()`. That is the AB-BA
//     shape `c7215eb` fixed on the OpenSL path -- `close` holding a lock while
//     waiting for a thread that wants the same lock. There is no such mutex
//     here: the writer owns the handle for its whole life, `close` asks it to
//     stop with an atomic and joins, and only then is anything freed.
//
// One capability is genuinely lost and ADR-023 records it rather than papering
// over it. An empty `CORDIAL_AUDIO_SINK` on PipeWire and PulseAudio means
// "follow whatever the session calls the default, and keep following it" -- a
// standing instruction, so changing the default sink mid-game moves the stream.
// ALSA resolves `default` once, inside alsa-lib's configuration, at
// `snd_pcm_open`. Nothing moves a running stream. The device string is not
// portable either: a PipeWire `node.name` is not a PCM name.

#include "pipewire_backend.h"

#include <atomic>
#include <cstdio>
#include <cstring>
#include <memory>
#include <thread>
#include <vector>

#ifdef CORDIAL_HAVE_ALSA

#include <alsa/asoundlib.h>
#include <dlfcn.h>

namespace cordial::audio {
namespace {

/// Everything this file calls in `libasound`, resolved once.
///
/// Spelled out rather than macro-generated so a missing symbol names itself
/// instead of failing as a null call -- the same reason `pulse_backend.cpp`
/// does it this way.
struct Alsa {
    int (*open)(snd_pcm_t**, const char*, snd_pcm_stream_t, int);
    int (*close)(snd_pcm_t*);
    int (*prepare)(snd_pcm_t*);
    int (*drop)(snd_pcm_t*);
    int (*nonblock)(snd_pcm_t*, int);
    snd_pcm_sframes_t (*writei)(snd_pcm_t*, const void*, snd_pcm_uframes_t);
    int (*recover)(snd_pcm_t*, int, int);

    int (*hw_params_malloc)(snd_pcm_hw_params_t**);
    void (*hw_params_free)(snd_pcm_hw_params_t*);
    int (*hw_params_any)(snd_pcm_t*, snd_pcm_hw_params_t*);
    int (*hw_params_set_access)(snd_pcm_t*, snd_pcm_hw_params_t*, snd_pcm_access_t);
    int (*hw_params_set_format)(snd_pcm_t*, snd_pcm_hw_params_t*, snd_pcm_format_t);
    int (*hw_params_set_rate_near)(snd_pcm_t*, snd_pcm_hw_params_t*, unsigned int*, int*);
    int (*hw_params_set_channels_near)(snd_pcm_t*, snd_pcm_hw_params_t*, unsigned int*);
    int (*hw_params_set_period_size_near)(snd_pcm_t*, snd_pcm_hw_params_t*,
                                          snd_pcm_uframes_t*, int*);
    int (*hw_params_set_buffer_size_near)(snd_pcm_t*, snd_pcm_hw_params_t*, snd_pcm_uframes_t*);
    int (*hw_params)(snd_pcm_t*, snd_pcm_hw_params_t*);
    int (*hw_params_get_rate)(const snd_pcm_hw_params_t*, unsigned int*, int*);
    int (*hw_params_get_channels)(const snd_pcm_hw_params_t*, unsigned int*);
    int (*hw_params_get_period_size)(const snd_pcm_hw_params_t*, snd_pcm_uframes_t*, int*);
    const char* (*strerror)(int);
};

const Alsa* alsa() {
    static const Alsa* table = [] () -> const Alsa* {
        void* lib = ::dlopen("libasound.so.2", RTLD_LAZY | RTLD_LOCAL);
        if (!lib) {
            std::fprintf(stderr,
                "I/Cordial-ALSA            libasound.so.2 is not on this machine; the ALSA "
                "backend is unavailable.\n");
            return nullptr;
        }
        auto* t = new Alsa{};
        bool ok = true;
        auto need = [&] (auto& slot, const char* name) {
            void* sym = ::dlsym(lib, name);
            if (!sym) {
                std::fprintf(stderr,
                    "E/Cordial-ALSA            libasound.so.2 has no %s; refusing to use a "
                    "half-resolved backend.\n", name);
                ok = false;
                return;
            }
            slot = reinterpret_cast<std::remove_reference_t<decltype(slot)>>(sym);
        };
#define NEED(field, name) need(t->field, name)
        NEED(open, "snd_pcm_open");
        NEED(close, "snd_pcm_close");
        NEED(prepare, "snd_pcm_prepare");
        NEED(drop, "snd_pcm_drop");
        NEED(nonblock, "snd_pcm_nonblock");
        NEED(writei, "snd_pcm_writei");
        NEED(recover, "snd_pcm_recover");
        NEED(hw_params_malloc, "snd_pcm_hw_params_malloc");
        NEED(hw_params_free, "snd_pcm_hw_params_free");
        NEED(hw_params_any, "snd_pcm_hw_params_any");
        NEED(hw_params_set_access, "snd_pcm_hw_params_set_access");
        NEED(hw_params_set_format, "snd_pcm_hw_params_set_format");
        NEED(hw_params_set_rate_near, "snd_pcm_hw_params_set_rate_near");
        NEED(hw_params_set_channels_near, "snd_pcm_hw_params_set_channels_near");
        NEED(hw_params_set_period_size_near, "snd_pcm_hw_params_set_period_size_near");
        NEED(hw_params_set_buffer_size_near, "snd_pcm_hw_params_set_buffer_size_near");
        NEED(hw_params, "snd_pcm_hw_params");
        NEED(hw_params_get_rate, "snd_pcm_hw_params_get_rate");
        NEED(hw_params_get_channels, "snd_pcm_hw_params_get_channels");
        NEED(hw_params_get_period_size, "snd_pcm_hw_params_get_period_size");
        NEED(strerror, "snd_strerror");
#undef NEED
        if (!ok) { delete t; return nullptr; }
        return t;
    }();
    return table;
}

class AlsaStream final : public OutputStream {
public:
    AlsaStream() = default;
    ~AlsaStream() override { close(); }

    bool open(uint32_t sample_bits, bool is_float, const char* node_description,
              const char* target_node_name, FillCallback cb, void* user) override;
    void close() override;
    bool is_open() const override { return pcm_ != nullptr; }

    void set_running(bool running) override { running_.store(running, std::memory_order_relaxed); }
    bool is_running() const override { return running_.load(std::memory_order_relaxed); }

    uint32_t rate_hz() const override { return rate_; }
    uint32_t channels() const override { return channels_; }
    uint32_t sample_bits() const override { return 32; }
    bool sample_is_float() const override { return true; }
    uint32_t burst_frames() const override { return period_; }
    uint64_t silence_cycles() const override { return silence_.load(std::memory_order_relaxed); }

private:
    void writer();

    snd_pcm_t* pcm_ = nullptr;
    std::thread thread_;
    std::vector<float> buffer_;

    FillCallback fill_ = nullptr;
    void* user_ = nullptr;
    uint32_t rate_ = 0;
    uint32_t channels_ = 0;
    uint32_t period_ = 0;

    std::atomic<bool> quit_{false};
    std::atomic<bool> running_{false};
    std::atomic<uint64_t> silence_{0};
};

/// The whole backend, on one thread that owns the device.
///
/// `snd_pcm_writei` blocking when the device is full *is* the pacing: there is
/// no callback to wait for and no sleep to tune, which is why this loop has
/// neither. The realtime rule from `pipewire_backend.h` applies to everything
/// reachable from the `fill_` call below, exactly as it does inside PipeWire's
/// `process()` -- no lock, no allocation, no logging on that path. The buffer
/// is sized once in `open` for that reason.
void AlsaStream::writer() {
    const Alsa* a = alsa();
    while (!quit_.load(std::memory_order_relaxed)) {
        const uint32_t frames = period_;
        if (running_.load(std::memory_order_relaxed) && fill_) {
            if (!fill_(buffer_.data(), frames, user_)) {
                // The engine asked to stop being pulled. Silence from here
                // rather than a teardown, for the same reason `set_running`
                // does: this may be reached from inside the engine's own
                // callback, and tearing the device down underneath it would be
                // the deadlock this file's header is about.
                running_.store(false, std::memory_order_relaxed);
            }
        } else {
            std::memset(buffer_.data(), 0, buffer_.size() * sizeof(float));
            silence_.fetch_add(1, std::memory_order_relaxed);
        }
        snd_pcm_sframes_t wrote = a->writei(pcm_, buffer_.data(), frames);
        if (wrote < 0) {
            // **Recover rather than give up.** An underrun is ordinary on a
            // desktop -- the scheduler was late once -- and a backend that
            // stops on the first one is a backend that stops. `snd_pcm_recover`
            // handles EPIPE and ESTRPIPE and returns the error untouched when
            // it is neither, which is the only case worth reporting.
            int err = a->recover(pcm_, static_cast<int>(wrote), 1 /* silent */);
            if (err < 0) {
                std::fprintf(stderr,
                    "E/Cordial-ALSA            write failed and could not recover: %s; the "
                    "stream is stopping.\n", a->strerror(err));
                return;
            }
        }
    }
}

bool AlsaStream::open(uint32_t, bool, const char*, const char* target_node_name,
                      FillCallback cb, void* user) {
    const Alsa* a = alsa();
    if (!a) return false;
    close();

    fill_ = cb;
    user_ = user;

    // A PCM name, not a PipeWire node name -- see the header. `default` is
    // alsa-lib's own configured default, which on a machine running PipeWire is
    // usually its ALSA plugin, and on one that is not is the hardware.
    const char* device = (target_node_name && target_node_name[0]) ? target_node_name : "default";
    // Opened non-blocking, then switched back. A blocking `snd_pcm_open`
    // sleeps in the kernel until whoever holds a hardware PCM lets go, so a
    // busy device used to wedge the engine thread at startup indefinitely;
    // non-blocking turns that into EBUSY, which is "unavailable".
    int err = a->open(&pcm_, device, SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    if (err >= 0) {
        err = a->nonblock(pcm_, 0);
        if (err < 0) { a->close(pcm_); }
    }
    if (err < 0) {
        std::fprintf(stderr,
            "I/Cordial-ALSA            could not open PCM \"%s\": %s; this backend is "
            "unavailable for this run.\n", device, a->strerror(err));
        pcm_ = nullptr;
        return false;
    }

    snd_pcm_hw_params_t* params = nullptr;
    if (a->hw_params_malloc(&params) < 0) { close(); return false; }

    // Asked for, then read back. **Every `_near` call may change the value it
    // is given**, and what the engine is told has to be what the device
    // granted -- not what this file would have preferred.
    unsigned int rate = 48000;
    unsigned int channels = 2;
    snd_pcm_uframes_t period = 1024;
    snd_pcm_uframes_t buffer_frames = period * 4;
    bool ok = a->hw_params_any(pcm_, params) >= 0
        && a->hw_params_set_access(pcm_, params, SND_PCM_ACCESS_RW_INTERLEAVED) >= 0
        && a->hw_params_set_format(pcm_, params, SND_PCM_FORMAT_FLOAT_LE) >= 0
        && a->hw_params_set_rate_near(pcm_, params, &rate, nullptr) >= 0
        && a->hw_params_set_channels_near(pcm_, params, &channels) >= 0
        && a->hw_params_set_period_size_near(pcm_, params, &period, nullptr) >= 0
        && a->hw_params_set_buffer_size_near(pcm_, params, &buffer_frames) >= 0
        && a->hw_params(pcm_, params) >= 0;

    if (ok) {
        unsigned int got_rate = 0, got_channels = 0;
        snd_pcm_uframes_t got_period = 0;
        a->hw_params_get_rate(params, &got_rate, nullptr);
        a->hw_params_get_channels(params, &got_channels);
        a->hw_params_get_period_size(params, &got_period, nullptr);
        rate_ = got_rate;
        channels_ = got_channels;
        period_ = static_cast<uint32_t>(got_period);
    }
    a->hw_params_free(params);

    if (!ok || rate_ == 0 || channels_ == 0 || period_ == 0) {
        std::fprintf(stderr,
            "E/Cordial-ALSA            \"%s\" would not take float32 interleaved at any rate "
            "this backend can offer.\n", device);
        close();
        return false;
    }

    if (a->prepare(pcm_) < 0) { close(); return false; }

    // Sized once, here, because the writer may not allocate.
    buffer_.assign(static_cast<size_t>(period_) * channels_, 0.0f);
    quit_.store(false, std::memory_order_relaxed);
    thread_ = std::thread([this] { writer(); });

    std::fprintf(stderr,
        "I/Cordial-ALSA            opened %u Hz, %u channel(s), PCM_FLOAT, %u frames per "
        "period, on \"%s\". Rate and channels are what the device granted, not what was "
        "asked for.\n", rate_, channels_, period_, device);
    return true;
}

void AlsaStream::close() {
    const Alsa* a = alsa();
    running_.store(false, std::memory_order_relaxed);
    quit_.store(true, std::memory_order_relaxed);
    if (thread_.joinable()) {
        // **Joined before anything is freed, and nothing is held while
        // joining.** The writer owns `pcm_` for its whole life; a lock shared
        // between it and this function is the AB-BA deadlock the header names.
        thread_.join();
    }
    if (pcm_ && a) {
        a->drop(pcm_);
        a->close(pcm_);
    }
    pcm_ = nullptr;
    buffer_.clear();
    buffer_.shrink_to_fit();
    fill_ = nullptr;
    user_ = nullptr;
}

} // namespace

bool alsa_available() {
    // Opening and closing a PCM is the only thing that proves a device is
    // there, and ADR-023 requires a probe this trustworthy because
    // `supportsAAudio()` is a one-way door: a backend that says yes and then
    // cannot open leaves the session silent with no fallback. It is not free
    // and it can fail transiently, which is exactly why it is cached.
    static const bool ok = [] {
        const Alsa* a = alsa();
        if (!a) return false;
        snd_pcm_t* pcm = nullptr;
        // Non-blocking for the reason `AlsaStream::open` gives: a busy device
        // is an answer, not a wait.
        if (a->open(&pcm, "default", SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK) < 0) return false;
        a->close(pcm);
        return true;
    }();
    return ok;
}

std::unique_ptr<OutputStream> make_alsa_stream() { return std::make_unique<AlsaStream>(); }

} // namespace cordial::audio

#else // !CORDIAL_HAVE_ALSA

namespace cordial::audio {

// Built without alsa-lib-devel. Honest rather than absent, so the selector can
// tell a run that asked for ALSA that it did not get it.
bool alsa_available() { return false; }
std::unique_ptr<OutputStream> make_alsa_stream() { return nullptr; }

} // namespace cordial::audio

#endif // CORDIAL_HAVE_ALSA
