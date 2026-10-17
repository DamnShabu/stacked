// Roblox's Android audio surface, answered from PipeWire.
//
// This is the Java half of audio. The native half — `libOpenSLES.so`, which
// FMOD uses for game and UI sound — lives in `opensles.cpp` and reaches
// PipeWire through the same backend; nothing here duplicates it. What is here
// are the two things that are *not* OpenSL ES and are easy to assume are:
//
// 1. **Device enumeration is Java.** Roblox asks
//    `AudioManager.getDevices(int)` for an `AudioDeviceInfo[]` and reads seven
//    getters off each element. `SLOutputMixItf::GetDestinationOutputDeviceIDs`
//    exists in `opensles.cpp` and reports zero devices, which is correct — the
//    OpenSL routing surface genuinely is not how Android enumerates audio
//    hardware, and it is not where a device picker gets its list.
//
// 2. **The microphone has two possible doors, and this file is one of them.**
//    This comment used to say the other one did not exist — that Roblox records
//    only through `org.webrtc.voiceengine.WebRtcAudioRecord`, so
//    `SLEngineItf::CreateAudioRecorder` could refuse forever. That was an
//    inference from the dex declaring a WebRTC surface, and
//    `docs/analysis/undefined-symbols.tsv` disagrees: `libroblox.so` links
//    `SL_IID_RECORD`. `opensles.cpp` now implements the OpenSL recorder, with
//    the microphone's lifetime tied to `SLRecordItf`'s record state exactly as
//    the rule below requires. Two doors, then — and both are shut by the same
//    rule rather than one of them being shut by not existing.
//
// Every method below was taken from the shipping dex with `dexproto.py`, not
// from memory of the Android SDK. The seven `AudioDeviceInfo` getters, the
// `getDevices(I)[Landroid/media/AudioDeviceInfo;` shape, the
// `AudioRecord.<init>(IIIII)` argument count and
// `AudioRecord.read(Ljava/nio/ByteBuffer;I)I` rather than the `byte[]`
// overload are all declared surface, and anything not declared there is not
// implemented here.
//
// -------------------------------------------------------------------------
// THE MICROPHONE RULE, which the rest of this file is arranged around
// -------------------------------------------------------------------------
//
// No PipeWire capture stream may exist while Roblox is not recording.
//
// Not paused, not muted, not connected-but-inactive — absent. A capture stream
// in any of those states still keeps the desktop's microphone indicator lit
// and still shows every other application that Cordial is holding the capture
// device, which is the harm; whether samples are flowing is not the part a
// user can see. `CaptureStream::close()` therefore destroys the `pw_stream`
// rather than deactivating it.
//
// There are exactly four callers of `CaptureStream::open()` in Cordial, three of
// them named here and the AAudio input stream in `aaudio.cpp` the fourth:
// `AudioRecord::startRecording` and `WebRtcAudioRecord::startRecording` below,
// and `AudioRecorderObject::start_capture` in `opensles.cpp`, which runs only
// from `SLRecordItf::SetRecordState(SL_RECORDSTATE_RECORDING)`. All three close
// on stop, on pause (the OpenSL path only — neither `AudioRecord` nor
// `WebRtcAudioRecord` has a pause state to close on) and on destroy. The
// OpenSL half of that was observed against PipeWire's own registry on
// 2026-08-02: no capture node existed while the recorder was merely realized,
// one appeared within half a second of `RECORDING`, and it was gone again on
// `PAUSED`, on `STOPPED` and after `Destroy` — including when the stop came
// from inside the buffer callback. Re-run on 2026-08-28 against this same
// `audio_probe.cpp` OpenSL path — the identical `CaptureStream` primitive
// `WebRtcAudioRecord::startRecording`/`stop` call below, not a second one —
// it held again: a `pw-dump` sampler independently caught the node while
// `SL_RECORDSTATE_RECORDING` and not otherwise, and 590 buffers of real
// samples were read across two record/pause cycles, proving data flowed
// rather than the stream merely existing. A real `cordial-run` session with
// this change built in, 25 s at Roblox's Landing screen with no voice call
// joined, showed zero `cordial-`-named nodes in `pw-dump` throughout. What
// that does not cover — `WebRtcAudioRecord`'s own JNI glue actually being
// driven by the engine end to end — needs a live voice call; see this class's
// own comment and the report accompanying the change that added it.
//
// Two consequences worth stating because they are the kind of thing a later
// change makes by accident:
//
// * **Enumeration must not open anything.** Listing a microphone is not using
//   one. `enumerate_devices()` walks the PipeWire registry and reads
//   properties; it never constructs a `CaptureStream`. If a device picker ever
//   starts lighting the microphone indicator, that invariant is what broke.
//   `stacked audio-inputs` is that device picker: `cordial_audio_sources`
//   lists, the `audio_input` config key stores a `node.name` that reaches the
//   client as `CORDIAL_AUDIO_SOURCE`, and neither creates a stream. A chosen
//   source is applied only where one of the callers above opens its stream.
//
// * **Failure closes.** Every path below that cannot honour the rule reports
//   failure instead of opening the microphone anyway — including the case
//   where recording starts successfully but the engine's own callback cannot
//   be reached, where the stream is closed again immediately rather than left
//   open delivering samples to nobody. A microphone held open for a recording
//   that is not happening is exactly the state this file exists to prevent.

#include <jni.h>
#include <jnivm.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "aaudio.h"
#include "pipewire_backend.h"

namespace cordial {

// Defined in `jni_shim.cpp`. Declared here, outside the anonymous namespace
// below, because a declaration written *inside* it would name a distinct,
// unique-per-translation-unit symbol rather than the real
// externally-linked `cordial::process_env` -- a link error waiting to
// happen the day `WebRtcAudioTrack`'s pump thread below actually needs it,
// rather than the compile error it should be.
jnivm::ENV* process_env();

namespace {

using jnivm::Class;
using jnivm::ENV;
using jnivm::Object;
using String = jnivm::String;

inline std::shared_ptr<String> str(const std::string& v) {
    return std::make_shared<String>(v);
}

/// `getProductName()` is declared `()Ljava/lang/CharSequence;` in the dex, not
/// `()Ljava/lang/String;`, and libjnivm derives the descriptor it matches a
/// hook by from the hook's own C++ types. A `getProductName` returning
/// `shared_ptr<String>` therefore registers as `()Ljava/lang/String;` and
/// never matches the call Roblox compiled — which is not a build error and not
/// a run-time error either. It is a device list where every entry has a null
/// name, which is exactly what the first run of this file produced:
///
///     [0] getId=220 getType=9 isSource=false getProductName="(null)"
///
/// Deriving from `jnivm::String` rather than from `Object` is what makes the
/// returned object still be a real Java string to everything downstream —
/// `GetStringUTFChars` and friends recover it by `dynamic_cast`, and a
/// `CharSequence` that was not one would be a null name again by a longer
/// route. This is the same trick `accessibility.cpp` uses for the parameter
/// direction, in the one direction that file did not need.
class CharSequence : public String {
public:
    explicit CharSequence(const std::string& v) : String(v) {}
    static void Register(ENV* env) { env->GetClass<CharSequence>("java/lang/CharSequence"); }
};

inline std::shared_ptr<CharSequence> chars(const std::string& v) {
    return std::make_shared<CharSequence>(v);
}

// ------------------------------------------------- android.media constants
//
// From AOSP's public `android.media` API — the documented constant values
// every Android application is compiled against, not anything read out of
// Roblox. Written out rather than included because there is no Android SDK in
// this tree to include them from.

constexpr jint GET_DEVICES_INPUTS = 0x0001;
constexpr jint GET_DEVICES_OUTPUTS = 0x0002;

constexpr jint TYPE_UNKNOWN = 0;
constexpr jint TYPE_BUILTIN_SPEAKER = 2;
constexpr jint TYPE_WIRED_HEADSET = 3;
constexpr jint TYPE_WIRED_HEADPHONES = 4;
constexpr jint TYPE_BLUETOOTH_SCO = 7;
constexpr jint TYPE_BLUETOOTH_A2DP = 8;
constexpr jint TYPE_HDMI = 9;
constexpr jint TYPE_USB_DEVICE = 11;
constexpr jint TYPE_BUILTIN_MIC = 15;
constexpr jint TYPE_USB_HEADSET = 22;

constexpr jint ENCODING_PCM_16BIT = 2;

constexpr jint AUDIO_RECORD_STATE_UNINITIALIZED = 0;
constexpr jint AUDIO_RECORD_STATE_INITIALIZED = 1;
constexpr jint RECORDSTATE_STOPPED = 1;
constexpr jint RECORDSTATE_RECORDING = 3;

/// Android's own default and the rate WebRTC's voice engine asks for. Used
/// wherever a rate has to be reported before a stream exists to ask.
constexpr jint DEFAULT_SAMPLE_RATE = 48000;

bool contains(const std::string& haystack, const char* needle) {
    return haystack.find(needle) != std::string::npos;
}

/// Lowercased copy, so the name matching below does not have to guess at
/// PipeWire's capitalisation (`HiFi__HDMI1__sink`, `hdmi-stereo`, `Speaker`).
std::string lower(const std::string& s) {
    std::string out = s;
    for (char& c : out) {
        if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    }
    return out;
}

/// The one genuinely interpretive step in this file: PipeWire describes a node
/// by how it is plugged in, Android describes it by what it is, and the two
/// vocabularies do not line up exactly.
///
/// The rule applied here is to answer from the most authoritative signal
/// available and, where none of them settles it, to answer `TYPE_UNKNOWN`
/// rather than the most plausible-sounding constant. `TYPE_BUILTIN_SPEAKER` is
/// a specific claim about hardware; guessing it for anything unrecognised
/// would be wrong roughly as often as it was right, and a device picker
/// showing a confidently wrong icon is harder to distrust than one showing an
/// honestly generic one.
///
/// The signals, most trustworthy first:
///
/// * `object.path`'s prefix names the SPA plugin that created the node —
///   `bluez5:` cannot be anything but Bluetooth, and unlike a name it is not
///   something a user can rename.
/// * `node.name` follows PipeWire's stable device-naming convention, in which
///   `.usb-` and `.pci-` are the bus and the `HiFi__<profile>__sink` tail is
///   the ALSA UCM profile. `hdmi`, `speaker`, `headphone`, `headset` and `mic`
///   appearing there are conventions, not guarantees, which is why they rank
///   below `object.path`.
/// * Direction, which decides between the paired Bluetooth constants: A2DP is
///   the playback profile and SCO the (bidirectional) headset one, so a
///   Bluetooth *source* is SCO by construction.
jint android_device_type(const audio::DeviceInfo& d) {
    const std::string path = lower(d.object_path);
    const std::string name = lower(d.node_name);
    const std::string nick = lower(d.nick);

    if (path.rfind("bluez5:", 0) == 0 || contains(name, "bluez_")) {
        return d.is_source ? TYPE_BLUETOOTH_SCO : TYPE_BLUETOOTH_A2DP;
    }
    if (contains(name, "hdmi") || contains(nick, "hdmi")) {
        // Android has no "DisplayPort"; HDMI is the constant it routes
        // display audio through, and a DP output is the same path here.
        return TYPE_HDMI;
    }
    if (contains(name, ".usb-") || contains(path, "usb")) {
        if (contains(name, "headset") || contains(nick, "headset")) return TYPE_USB_HEADSET;
        return TYPE_USB_DEVICE;
    }
    if (contains(name, "headset") || contains(nick, "headset")) return TYPE_WIRED_HEADSET;
    if (contains(name, "headphone") || contains(nick, "headphone")) return TYPE_WIRED_HEADPHONES;
    if (d.is_source && (contains(name, "mic") || contains(nick, "mic"))) return TYPE_BUILTIN_MIC;
    if (!d.is_source && (contains(name, "speaker") || contains(nick, "speaker"))) {
        return TYPE_BUILTIN_SPEAKER;
    }
    // A line input, a virtual sink, a card whose UCM profile is named
    // something this does not recognise. Android documents TYPE_UNKNOWN for
    // exactly this and Roblox is entitled to see it rather than a flattering
    // guess.
    return TYPE_UNKNOWN;
}

// ------------------------------------------------ android.media.AudioDeviceInfo

/// `android.media.AudioDeviceInfo`
///
/// A snapshot, not a live view. Android's own `AudioDeviceInfo` is immutable
/// and describes the device as it was when `getDevices` returned it, so
/// copying PipeWire's answer into the object at construction is the faithful
/// translation as well as the simple one: a getter that re-queried the session
/// could report a different device than the one the caller thinks it is
/// holding.
class AudioDeviceInfo : public Object {
public:
    jint id = 0;
    jint type = TYPE_UNKNOWN;
    bool source = false;
    std::string productName;

    /// PipeWire converts on the graph edge, so a device's *native* rate is not
    /// the set of rates a stream may ask for. These are the rates Cordial will
    /// actually accept from Roblox and honour, which is what the getter is
    /// asked to describe.
    static std::shared_ptr<jnivm::Array<jint>> rates(ENV*, Object*) {
        static const jint kRates[] = {8000, 11025, 16000, 22050, 32000, 44100, 48000};
        auto out = std::make_shared<jnivm::Array<jint>>(
            static_cast<jsize>(sizeof(kRates) / sizeof(kRates[0])));
        std::memcpy(out->getArray(), kRates, sizeof(kRates));
        return out;
    }

    static std::shared_ptr<jnivm::Array<jint>> channels(ENV*, Object*) {
        static const jint kCounts[] = {1, 2};
        auto out = std::make_shared<jnivm::Array<jint>>(2);
        std::memcpy(out->getArray(), kCounts, sizeof(kCounts));
        return out;
    }

    /// One entry, and deliberately only one. `CaptureStream` negotiates
    /// `SPA_AUDIO_FORMAT_S16` and nothing else, and `PlaybackStream` reports
    /// failure for a layout it cannot describe rather than reinterpreting the
    /// bytes; advertising a float or 8-bit encoding here would be a promise
    /// made in this file that the backend breaks in another.
    static std::shared_ptr<jnivm::Array<jint>> encodings(ENV*, Object*) {
        auto out = std::make_shared<jnivm::Array<jint>>(1);
        out->getArray()[0] = ENCODING_PCM_16BIT;
        return out;
    }

    static jint getId(ENV*, Object* self) { return self_of(self)->id; }
    static jint getType(ENV*, Object* self) { return self_of(self)->type; }
    static jboolean isSource(ENV*, Object* self) { return self_of(self)->source; }

    /// PipeWire's `node.description` verbatim — "Raptor Lake-P/U/H cAVS
    /// Speaker", the same string the user's own volume control shows. The
    /// point of a device picker is that the user recognises the entry, so
    /// anything invented here (a tidied-up name, a Cordial-branded one) makes
    /// the list worse however much better it reads.
    static std::shared_ptr<CharSequence> getProductName(ENV*, Object* self) {
        return chars(self_of(self)->productName);
    }

    /// The same answer under the `()Ljava/lang/String;` descriptor. Registered
    /// alongside the declared `CharSequence` one for the reason
    /// `accessibility.cpp` registers both of its `setClassName` overloads: it
    /// costs one line, and the failure it prevents is silent.
    static std::shared_ptr<String> getProductNameStr(ENV*, Object* self) {
        return str(self_of(self)->productName);
    }

    static void Register(ENV* env) {
        env->GetClass<AudioDeviceInfo>("android/media/AudioDeviceInfo");
        auto c = env->GetClass("android/media/AudioDeviceInfo");
        c->HookInstanceFunction(env, "getId", &AudioDeviceInfo::getId);
        c->HookInstanceFunction(env, "getType", &AudioDeviceInfo::getType);
        c->HookInstanceFunction(env, "isSource", &AudioDeviceInfo::isSource);
        c->HookInstanceFunction(env, "getProductName", &AudioDeviceInfo::getProductName);
        c->HookInstanceFunction(env, "getProductName", &AudioDeviceInfo::getProductNameStr);
        c->HookInstanceFunction(env, "getSampleRates", &AudioDeviceInfo::rates);
        c->HookInstanceFunction(env, "getChannelCounts", &AudioDeviceInfo::channels);
        c->HookInstanceFunction(env, "getEncodings", &AudioDeviceInfo::encodings);
    }

private:
    static AudioDeviceInfo* self_of(Object* o) {
        static AudioDeviceInfo fallback;
        auto* d = dynamic_cast<AudioDeviceInfo*>(o);
        return d ? d : &fallback;
    }
};

std::shared_ptr<AudioDeviceInfo> make_device_info(const audio::DeviceInfo& d) {
    auto info = std::make_shared<AudioDeviceInfo>();
    info->id = static_cast<jint>(d.id);
    info->source = d.is_source;
    info->type = android_device_type(d);
    info->productName = !d.description.empty() ? d.description
                        : !d.nick.empty()      ? d.nick
                                                : d.node_name;
    return info;
}

/// The PipeWire node name of the current default source, or empty if the
/// session names none. Read at the moment recording starts rather than cached,
/// so that switching the desktop's default microphone between two Roblox voice
/// sessions is picked up without restarting the client.
std::string default_source_node_name() {
    for (const audio::DeviceInfo& d : audio::enumerate_devices()) {
        if (d.is_source && d.is_default) return d.node_name;
    }
    return {};
}

/// Where a recording that is about to open should connect: the source the
/// user chose (the `audio_input` config key, sent as `CORDIAL_AUDIO_SOURCE`) if
/// it is there, otherwise the default source as it is at this moment. Empty chosen means the second, which
/// is exactly what these two callers did before there was a microphone picker.
/// Enumeration only -- nothing here opens a stream.
std::string source_node_name_for_recording() {
    std::string chosen = audio::resolve_input_target(audio::configured_input_device());
    return chosen.empty() ? default_source_node_name() : chosen;
}

// ---------------------------------------------------- android.media.AudioManager

/// `android.media.AudioManager`
///
/// Only the methods the dex declares, and among those only the ones that have
/// a truthful answer on this host. Everything to do with Bluetooth SCO
/// (`startBluetoothSco`, `setBluetoothScoOn`, `isBluetoothScoOn`) is left
/// unimplemented rather than answered `false`: SCO is a telephony routing mode
/// Android's audio flinger owns, PipeWire has no equivalent to report on, and
/// a confident `false` would be a claim about hardware state rather than an
/// admission of not knowing.
class AudioManager : public Object {
public:
    /// `getDevices(int flags)` — the device list, and the reason this class
    /// exists.
    ///
    /// The default device is placed first within its direction. Android's
    /// `AudioDeviceInfo` has no `isDefault()` and the dex does not declare
    /// one, so ordering is the only channel available for saying which device
    /// the host would pick.
    ///
    /// **`INFERRED`**: that a caller taking element zero therefore gets the
    /// host's default is a hope about the caller, not an observed fact about
    /// this one. Nothing has been seen calling `getDevices` yet (the one-shot
    /// line below exists to catch it when something does), and an earlier
    /// revision of this comment asserted that AOSP's own implementation orders
    /// the array this way, which is not something this project has any way to
    /// check and should not have been written as established.
    ///
    /// What *is* established is the routing, which does not depend on this
    /// ordering at all: `PlaybackStream` connects with `PW_ID_ANY` and
    /// autoconnect, so audio goes wherever PipeWire's default sink is at the
    /// time — measured on 2026-08-02 by capturing that sink's monitor while a
    /// tone was pushed through `SLAndroidSimpleBufferQueueItf::Enqueue`.
    static std::shared_ptr<jnivm::Array<AudioDeviceInfo>> getDevices(ENV*, Object*, jint flags) {
        const bool want_inputs = (flags & GET_DEVICES_INPUTS) != 0;
        const bool want_outputs = (flags & GET_DEVICES_OUTPUTS) != 0;

        std::vector<std::shared_ptr<AudioDeviceInfo>> chosen;
        std::vector<audio::DeviceInfo> devices = audio::enumerate_devices();
        // Two passes so defaults lead, without sorting the vector and
        // disturbing the order PipeWire reported the rest in.
        for (bool defaults_pass : {true, false}) {
            for (const audio::DeviceInfo& d : devices) {
                if (d.is_default != defaults_pass) continue;
                if (d.is_source && !want_inputs) continue;
                if (!d.is_source && !want_outputs) continue;
                chosen.push_back(make_device_info(d));
            }
        }

        auto out = std::make_shared<jnivm::Array<AudioDeviceInfo>>(static_cast<jsize>(chosen.size()));
        for (size_t i = 0; i < chosen.size(); ++i) {
            (*out)[static_cast<jint>(i)] = chosen[i];
        }
        // Once, and only the first time. Whether Roblox asks for the device
        // list at all is a question `--dump-classes` cannot answer — the dump
        // lists this class because Cordial registered it, not because anything
        // called it — and it is the difference between a device list that is
        // used and one that is merely correct.
        static bool announced = false;
        if (!announced) {
            announced = true;
            std::fprintf(stderr,
                "I/Cordial-Audio           AudioManager.getDevices(0x%X) called by the engine; "
                "answering with %zu of %zu PipeWire device(s), default first.\n",
                flags, chosen.size(), devices.size());
        }
        return out;
    }

    /// `getCommunicationDevice()` — Android's name for "the device voice
    /// audio is going to and coming from right now". PipeWire's default
    /// source is the honest answer: it is what a capture stream that names no
    /// target will be connected to, which is precisely what
    /// `CaptureStream::open` does when Roblox starts recording.
    ///
    /// Null when the session names no default, rather than the first source
    /// that happens to exist. A caller that gets null knows it does not know;
    /// a caller handed an arbitrary device does not.
    static std::shared_ptr<AudioDeviceInfo> getCommunicationDevice(ENV*, Object*) {
        static bool announced = false;
        if (!announced) {
            announced = true;
            std::fprintf(stderr,
                "I/Cordial-Audio           AudioManager.getCommunicationDevice called by the "
                "engine.\n");
        }
        for (const audio::DeviceInfo& d : audio::enumerate_devices()) {
            if (d.is_source && d.is_default) return make_device_info(d);
        }
        return nullptr;
    }

    static jboolean isMicrophoneMute(ENV*, Object*) {
        // Cordial has no global microphone mute of its own, and reporting the
        // *host's* mute state would be reporting on a control Roblox cannot
        // then operate through `setMicrophoneMute`. False is the truthful
        // answer to "is Cordial muting the microphone", which is the question
        // this object is being asked.
        return false;
    }

    static jboolean isVolumeFixed(ENV*, Object*) {
        // The host's mixer is adjustable, per stream, and `SLVolumeItf`
        // already routes Roblox's own volume changes to it.
        return false;
    }

    static jboolean isSpeakerphoneOn(ENV*, Object*) {
        // A desktop has no earpiece to be the alternative to a speakerphone,
        // so audio is always going to what Android would call the speaker.
        return true;
    }

    static jboolean isMusicActive(ENV*, Object*) {
        // Would require watching every other client's streams in the session
        // to answer, which is a great deal of machinery for a hint Roblox uses
        // to duck its own audio. False is the conservative answer: it makes
        // Roblox play normally rather than quietly for a reason the user
        // cannot see.
        return false;
    }

    static void Register(ENV* env) {
        env->GetClass<AudioManager>("android/media/AudioManager");
        auto c = env->GetClass("android/media/AudioManager");
        c->HookInstanceFunction(env, "getDevices", &AudioManager::getDevices);
        c->HookInstanceFunction(env, "getCommunicationDevice", &AudioManager::getCommunicationDevice);
        c->HookInstanceFunction(env, "isMicrophoneMute", &AudioManager::isMicrophoneMute);
        c->HookInstanceFunction(env, "isVolumeFixed", &AudioManager::isVolumeFixed);
        c->HookInstanceFunction(env, "isSpeakerphoneOn", &AudioManager::isSpeakerphoneOn);
        c->HookInstanceFunction(env, "isMusicActive", &AudioManager::isMusicActive);
    }
};

// ----------------------------------------------------- android.media.AudioRecord

/// `android.media.AudioRecord`
///
/// The gate itself. Constructing one of these opens nothing; only
/// `startRecording()` does, and `stop()` and `release()` both close.
///
/// `getMinBufferSize` is declared static in the dex and is answered without
/// any object existing, which is worth noting because it is the one method
/// here that a caller reaches before deciding whether to record at all —
/// another reason it must not touch the microphone.
class AudioRecord : public Object {
public:
    audio::CaptureStream capture;
    jint sampleRate = DEFAULT_SAMPLE_RATE;
    jint channelCount = 1;
    jint bufferSizeBytes = 0;
    std::atomic<jint> recordState{RECORDSTATE_STOPPED};
    std::string targetNode;

    /// `AudioRecord(int audioSource, int sampleRateInHz, int channelConfig,
    ///              int audioFormat, int bufferSizeInBytes)`
    ///
    /// `channelConfig` arrives as `CHANNEL_IN_MONO` (0x10) or
    /// `CHANNEL_IN_STEREO` (0x0c) rather than a count, which is the one place
    /// this constructor's arguments are not what their names suggest.
    static std::shared_ptr<AudioRecord> ctor(ENV*, Class*, jint /*audioSource*/, jint sampleRateInHz,
                                              jint channelConfig, jint audioFormat,
                                              jint bufferSizeInBytes) {
        auto r = std::make_shared<AudioRecord>();
        // Worth a line every time rather than once: each construction is a
        // recorder that could go on to open the microphone, and a log that
        // shows one of them and hides the rest is a log nobody can count with.
        // Constructing one opens nothing; only `startRecording` does.
        std::fprintf(stderr,
            "I/Cordial-Audio           android.media.AudioRecord constructed (%d Hz, "
            "channelConfig 0x%X, format %d); no capture stream until startRecording.\n",
            sampleRateInHz, channelConfig, audioFormat);
        r->sampleRate = sampleRateInHz > 0 ? sampleRateInHz : DEFAULT_SAMPLE_RATE;
        r->channelCount = channelConfig == 0x0c ? 2 : 1;
        r->bufferSizeBytes = bufferSizeInBytes > 0 ? bufferSizeInBytes : 4096;
        if (audioFormat != ENCODING_PCM_16BIT) {
            // Reported now, at construction, rather than by producing silence
            // later. `getState()` staying UNINITIALIZED is how AudioRecord
            // says this, and every caller checks it before recording.
            std::fprintf(stderr,
                "E/Cordial-Audio           AudioRecord asked for encoding %d; only "
                "ENCODING_PCM_16BIT (2) is supported, so this recorder reports "
                "STATE_UNINITIALIZED and will not open the microphone.\n", audioFormat);
            r->sampleRate = 0; // makes getState() report UNINITIALIZED, below
        }
        return r;
    }

    static jint getMinBufferSize(ENV*, Class*, jint sampleRateInHz, jint channelConfig,
                                  jint /*audioFormat*/) {
        const jint channels = channelConfig == 0x0c ? 2 : 1;
        const jint rate = sampleRateInHz > 0 ? sampleRateInHz : DEFAULT_SAMPLE_RATE;
        // 20 ms of S16 frames, matching the callback period WebRTC's voice
        // engine uses. Android returns a hardware-derived figure here; there
        // is no hardware period to derive one from when PipeWire owns the
        // graph, so this is the smallest buffer the rest of this file is
        // actually prepared to keep fed.
        return channels * 2 * rate / 50;
    }

    static jint getState(ENV*, Object* self) {
        auto* r = as(self);
        return (r && r->sampleRate > 0) ? AUDIO_RECORD_STATE_INITIALIZED
                                         : AUDIO_RECORD_STATE_UNINITIALIZED;
    }
    static jint getRecordingState(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->recordState.load() : RECORDSTATE_STOPPED;
    }
    static jint getSampleRate(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->sampleRate : 0;
    }
    static jint getChannelCount(ENV*, Object* self) {
        auto* r = as(self);
        return r ? r->channelCount : 0;
    }
    static jint getAudioFormat(ENV*, Object*) { return ENCODING_PCM_16BIT; }
    static jint getBufferSizeInFrames(ENV*, Object* self) {
        auto* r = as(self);
        if (!r || r->channelCount == 0) return 0;
        return r->bufferSizeBytes / (r->channelCount * 2);
    }
    static jint getAudioSessionId(ENV*, Object*) {
        // Android uses this to tie effects (AEC, NS) to a recording session.
        // There are no such effects here to tie anything to, and zero is the
        // documented "no session" value rather than an id that would imply one.
        return 0;
    }

    /// **One of exactly two places in Cordial that open the microphone.**
    static void startRecording(ENV*, Object* self) {
        auto* r = as(self);
        if (!r || r->sampleRate <= 0) return;
        if (r->capture.is_open()) return;
        r->targetNode = source_node_name_for_recording();
        if (!r->capture.open(static_cast<uint32_t>(r->sampleRate),
                             static_cast<uint32_t>(r->channelCount), r->targetNode)) {
            // Stays STOPPED. A recorder that reports RECORDING with no stream
            // behind it is the "stub that lies" this project keeps finding the
            // cost of: the caller would sit reading zero bytes forever with
            // nothing to explain why.
            std::fprintf(stderr,
                "E/Cordial-Audio           AudioRecord.startRecording could not open a "
                "capture stream; staying stopped rather than reporting a recording that "
                "is not happening.\n");
            return;
        }
        r->recordState.store(RECORDSTATE_RECORDING);
    }

    static void stop(ENV*, Object* self) {
        auto* r = as(self);
        if (!r) return;
        r->recordState.store(RECORDSTATE_STOPPED);
        r->capture.close();
    }

    static void release(ENV*, Object* self) {
        // Android permits release() without a preceding stop(), and a
        // released recorder must not be holding the device either.
        // CaptureStream::close() is idempotent, so the ordinary
        // stop()-then-release() pair costs nothing extra.
        stop(nullptr, self);
    }

    /// `read(ByteBuffer audioBuffer, int sizeInBytes)`
    static jint read(ENV*, Object* self, std::shared_ptr<jnivm::ByteBuffer> buffer,
                      jint sizeInBytes) {
        auto* r = as(self);
        if (!r || !buffer || sizeInBytes <= 0) return 0;
        void* dst = buffer->buffer;
        if (!dst) return 0;
        const jlong capacity = buffer->capacity;
        const uint32_t want =
            static_cast<uint32_t>(sizeInBytes < capacity ? sizeInBytes : capacity);
        return static_cast<jint>(r->capture.read(dst, want));
    }

    static void Register(ENV* env) {
        env->GetClass<AudioRecord>("android/media/AudioRecord");
        auto c = env->GetClass("android/media/AudioRecord");
        c->Hook(env, "<init>", &AudioRecord::ctor);
        c->Hook(env, "getMinBufferSize", &AudioRecord::getMinBufferSize);
        c->HookInstanceFunction(env, "getState", &AudioRecord::getState);
        c->HookInstanceFunction(env, "getRecordingState", &AudioRecord::getRecordingState);
        c->HookInstanceFunction(env, "getSampleRate", &AudioRecord::getSampleRate);
        c->HookInstanceFunction(env, "getChannelCount", &AudioRecord::getChannelCount);
        c->HookInstanceFunction(env, "getAudioFormat", &AudioRecord::getAudioFormat);
        c->HookInstanceFunction(env, "getBufferSizeInFrames", &AudioRecord::getBufferSizeInFrames);
        c->HookInstanceFunction(env, "getAudioSessionId", &AudioRecord::getAudioSessionId);
        c->HookInstanceFunction(env, "startRecording", &AudioRecord::startRecording);
        c->HookInstanceFunction(env, "stop", &AudioRecord::stop);
        c->HookInstanceFunction(env, "release", &AudioRecord::release);
        c->HookInstanceFunction(env, "read", &AudioRecord::read);
    }

private:
    static AudioRecord* as(Object* o) { return dynamic_cast<AudioRecord*>(o); }
};

// ---------------------------------------------------------- WebRTC JNI plumbing
//
// Two things the three classes below share, lifted out here because
// `WebRtcAudioManager`, `WebRtcAudioRecord` and `WebRtcAudioTrack` all need
// both and a third copy is how a helper drifts out of sync with itself.

/// Convert a C++ object into the `jobject` libjnivm expects, the way
/// `init_params.cpp` does and for the same reason its own comment gives: a
/// bare `Object*` has no `clazz` set, `GetObjectClass` then returns null, and
/// every field or method lookup on it resolves against nothing. Needed here
/// because `WebRtcAudioTrack`'s pump thread and `WebRtcAudioManager::init`
/// both have to hand their own `this` back to a raw native function pointer
/// as its `jobject thiz`.
template <class T>
static auto to_jni(ENV* env, const std::shared_ptr<T>& p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

/// Look up a native function pointer the *engine* registered on one of its
/// own classes via `RegisterNatives`, the way `cordial_game_activity_start` in
/// `game_activity.cpp` reaches AGDK's lifecycle natives.
///
/// This is not a style choice: `docs/analysis/jni-natives.tsv` -- the
/// exported-symbol table every other JNI binding in this codebase is checked
/// against -- has no `Java_org_webrtc_voiceengine_WebRtcAudioTrack_native...`
/// entry for either `nativeCacheDirectBufferAddress` or `nativeGetPlayoutData`
/// (nor a `nativeDataIsRecorded` for the record side), which means WebRTC's
/// own C++ audio glue binds them dynamically instead of exporting them under
/// the classic `javah` naming convention. libjnivm's ordinary method dispatch
/// deliberately excludes anything bound this way -- `AllowNative` is false for
/// a plain `GetMethodID`, and `CallVoidMethod` dispatches on `nativehandle`,
/// which `RegisterNatives` never sets -- so a hook cannot reach these through
/// `CallVoidMethod` and has to go through `Class::natives` directly instead,
/// exactly as `game_activity.cpp`'s own comment on this same trick explains.
///
/// Returns null, rather than asserting, when nothing has registered the name
/// yet: at the point `initPlayout` needs
/// `nativeCacheDirectBufferAddress`, whether the engine's C++ has already
/// called `RegisterNatives` on this class is a question this project has not
/// yet had a live voice call to answer, and a null return lets the caller
/// refuse cleanly rather than dereference nothing.
void* find_registered_native(ENV* env, const char* class_name, const char* method_name) {
    auto cls = env->GetClass(class_name);
    if (!cls) return nullptr;
    std::lock_guard<std::mutex> lock(cls->mtx);
    auto it = cls->natives.find(method_name);
    return it == cls->natives.end() ? nullptr : it->second;
}

// Defined after `WebRtcAudioManager`, forward-declared here so `init()` below
// can call it; see the definition for what is verified against the dex and
// what is recalled from WebRTC's public source. Returns whether the engine's
// audio device module actually received these parameters -- `init()` reports
// that back to the caller rather than a bare `true`, on the same stub-that-
// lies grounds `AGENTS.md` opens on: an `init()` that always answers success
// is exactly the shape of gap that stub-detection review looks for.
bool cache_audio_parameters(ENV* env, Object* self, jlong native_audio_manager);

// ------------------------------------- org.webrtc.voiceengine.WebRtcAudioManager

/// `org.webrtc.voiceengine.WebRtcAudioManager`
///
/// WebRTC's voice engine asks Java for the platform's audio parameters once,
/// at construction, and then builds its whole audio device module around the
/// answers. On real Android this class is Kotlin/Java running in the app; that
/// cannot execute under Cordial (there is no JVM — the same established
/// finding that governs `MainGameActivity.bootstrapTheApp`), so the class is
/// implemented here instead and answers from what Cordial can actually do.
///
/// **`init()` used to report failure, deliberately** — the parameter getters
/// were honest, but the uplink was only half the path: WebRTC also needed
/// `WebRtcAudioTrack` for the downlink, and an audio device module that had
/// been told initialisation succeeded would proceed to open a microphone in
/// order to send audio into a session it could not play the other side of.
///
/// **It now reports success**, because `WebRtcAudioTrack` below implements the
/// downlink and `WebRtcAudioRecord` below implements the uplink, and the
/// reason for refusing — an audio device module told to proceed into a
/// session it could not complete either half of — is gone from both
/// directions at once.
///
/// **Established, not assumed, and this is the part worth reading before
/// touching either half again.** Whether WebRTC's own C++ audio device module
/// has some other reason to treat either half as fatal to the other — the
/// same shape of one-way door `aaudio.cpp`'s header documents for FMOD, where
/// one refused `openStream` cost every subsequent path — is **UNVERIFIED**.
/// It needs a live voice call to answer and neither this change nor the one
/// that added `WebRtcAudioTrack` had one; see the report accompanying this
/// change for exactly what was and was not run. If a signed-in session with
/// voice chat active shows either direction never starting, that one-way-door
/// question is the first thing to check, ahead of anything in
/// `WebRtcAudioRecord` or `WebRtcAudioTrack` themselves.
class WebRtcAudioManager : public Object {
public:
    /// The engine's own handle from `<init>(J)V`, threaded back through
    /// `nativeCacheAudioParameters` below so its C++ side can find the object
    /// it belongs to. Silently discarded before this class hooked its own
    /// constructor — libjnivm builds a `WebRtcAudioManager` regardless of
    /// whether `<init>` is hooked, so the gap was invisible rather than a
    /// crash.
    jlong nativeAudioManager = 0;

    static std::shared_ptr<WebRtcAudioManager> ctor(ENV*, Class*, jlong native_audio_manager) {
        auto m = std::make_shared<WebRtcAudioManager>();
        m->nativeAudioManager = native_audio_manager;
        return m;
    }

    /// `init()`. Hands the engine's own audio device module the parameters it
    /// needs and reports back **whether that actually happened** — see
    /// `cache_audio_parameters`'s own comment for where the field order comes
    /// from and what in it is verified against the shipping dex versus
    /// recalled from WebRTC's public source.
    ///
    /// This used to return `true` regardless of `cache_audio_parameters`'
    /// outcome — found in review as the stub-that-lies shape `AGENTS.md`
    /// opens on: `nativeCacheAudioParameters` not yet being registered is a
    /// real, reachable gap (`cache_audio_parameters`'s own header names when),
    /// and an `init()` that says success anyway sends the engine's audio
    /// device module forward on parameters it never got, to fail somewhere
    /// with no visible relationship to the cause.
    static jboolean init(ENV* env, Object* self) {
        auto* m = as(self);
        const bool cached = cache_audio_parameters(env, self, m ? m->nativeAudioManager : 0);
        static bool said = false;
        if (!said) {
            said = true;
            std::fprintf(stderr,
                "I/Cordial-Audio           WebRtcAudioManager.init reports %s: WebRtcAudioTrack "
                "implements the downlink and WebRtcAudioRecord implements the uplink, so a voice "
                "session can both send and receive.\n", cached ? "success" : "failure");
        }
        return cached;
    }

    static void dispose(ENV*, Object*) {}

    static jint getNativeOutputSampleRate(ENV*, Object*) { return DEFAULT_SAMPLE_RATE; }
    static jint getSampleRateForApiLevel(ENV*, Object*) { return DEFAULT_SAMPLE_RATE; }

    // Mono in both directions: `CaptureStream` is opened mono for voice, and
    // claiming stereo would have WebRTC size its buffers for frames that never
    // arrive.
    static jboolean getStereoInput(ENV*, Class*) { return false; }
    static jboolean getStereoOutput(ENV*, Class*) { return false; }

    /// 10 ms at 48 kHz, WebRTC's own frame size. Not a hardware figure — there
    /// is no fixed hardware period to report when PipeWire negotiates the
    /// graph's quantum dynamically — but it is the period this backend is
    /// built to deliver, which is the honest thing for the caller to size
    /// itself against.
    static jint getLowLatencyInputFramesPerBuffer(ENV*, Object*) { return DEFAULT_SAMPLE_RATE / 100; }
    static jint getLowLatencyOutputFramesPerBuffer(ENV*, Object*) { return DEFAULT_SAMPLE_RATE / 100; }
    static jint getMinInputFrameSize(ENV*, Class*, jint sampleRate, jint channels) {
        return (sampleRate > 0 ? sampleRate : DEFAULT_SAMPLE_RATE) / 100 *
               (channels > 0 ? channels : 1);
    }
    static jint getMinOutputFrameSize(ENV*, Class*, jint sampleRate, jint channels) {
        return getMinInputFrameSize(nullptr, nullptr, sampleRate, channels);
    }

    // Android's hardware audio effects. There are none here — PipeWire's echo
    // canceller is a separate module the user opts into, not something Cordial
    // can claim on the device's behalf — and saying otherwise would have
    // WebRTC switch off its own software AEC in favour of one that does not
    // exist, which is audible as echo rather than as an error.
    static jboolean isAcousticEchoCancelerSupported(ENV*, Class*) { return false; }
    static jboolean isNoiseSuppressorSupported(ENV*, Class*) { return false; }
    static jboolean isLowLatencyInputSupported(ENV*, Object*) { return false; }
    static jboolean isLowLatencyOutputSupported(ENV*, Object*) { return false; }
    static jboolean isProAudioSupported(ENV*, Object*) { return false; }
    static jboolean isAAudioSupported(ENV*, Object*) { return false; }
    static jboolean hasEarpiece(ENV*, Object*) { return false; }
    static jboolean isCommunicationModeEnabled(ENV*, Object*) { return false; }
    static jboolean isDeviceBlacklistedForOpenSLESUsage(ENV*, Object*) { return false; }

    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioManager>("org/webrtc/voiceengine/WebRtcAudioManager");
        auto c = env->GetClass("org/webrtc/voiceengine/WebRtcAudioManager");
        c->Hook(env, "<init>", &WebRtcAudioManager::ctor);
    // Static on all of these, and the difference is not cosmetic. libjnivm binds
    // by descriptor: hooked as instance methods they registered cleanly and the
    // engine never reached one of them, so every answer below was a value
    // nothing read. Found by tools/hook_descriptors.py, which diffs each hook's
    // derived descriptor against the dex; the dex marks these ACC_STATIC.
        c->HookInstanceFunction(env, "init", &WebRtcAudioManager::init);
        c->HookInstanceFunction(env, "dispose", &WebRtcAudioManager::dispose);
        c->HookInstanceFunction(env, "getNativeOutputSampleRate",
                                &WebRtcAudioManager::getNativeOutputSampleRate);
        c->HookInstanceFunction(env, "getSampleRateForApiLevel",
                                &WebRtcAudioManager::getSampleRateForApiLevel);
        c->Hook(env, "getStereoInput", &WebRtcAudioManager::getStereoInput);
        c->Hook(env, "getStereoOutput", &WebRtcAudioManager::getStereoOutput);
        c->HookInstanceFunction(env, "getLowLatencyInputFramesPerBuffer",
                                &WebRtcAudioManager::getLowLatencyInputFramesPerBuffer);
        c->HookInstanceFunction(env, "getLowLatencyOutputFramesPerBuffer",
                                &WebRtcAudioManager::getLowLatencyOutputFramesPerBuffer);
        c->Hook(env, "getMinInputFrameSize",
                                &WebRtcAudioManager::getMinInputFrameSize);
        c->Hook(env, "getMinOutputFrameSize",
                                &WebRtcAudioManager::getMinOutputFrameSize);
        c->Hook(env, "isAcousticEchoCancelerSupported",
                                &WebRtcAudioManager::isAcousticEchoCancelerSupported);
        c->Hook(env, "isNoiseSuppressorSupported",
                                &WebRtcAudioManager::isNoiseSuppressorSupported);
        c->HookInstanceFunction(env, "isLowLatencyInputSupported",
                                &WebRtcAudioManager::isLowLatencyInputSupported);
        c->HookInstanceFunction(env, "isLowLatencyOutputSupported",
                                &WebRtcAudioManager::isLowLatencyOutputSupported);
        c->HookInstanceFunction(env, "isProAudioSupported", &WebRtcAudioManager::isProAudioSupported);
        c->HookInstanceFunction(env, "isAAudioSupported", &WebRtcAudioManager::isAAudioSupported);
        c->HookInstanceFunction(env, "hasEarpiece", &WebRtcAudioManager::hasEarpiece);
        c->HookInstanceFunction(env, "isCommunicationModeEnabled",
                                &WebRtcAudioManager::isCommunicationModeEnabled);
        c->HookInstanceFunction(env, "isDeviceBlacklistedForOpenSLESUsage",
                                &WebRtcAudioManager::isDeviceBlacklistedForOpenSLESUsage);
    }

private:
    static WebRtcAudioManager* as(Object* o) { return dynamic_cast<WebRtcAudioManager*>(o); }
};

/// `nativeCacheAudioParameters`, called once from `WebRtcAudioManager::init`.
///
/// **The argument count and every type are verified against the shipping
/// dex**: `tools/dex_method.py` reports
/// `WebRtcAudioManager.nativeCacheAudioParameters(IIIZZZZZZZIIJ)V` — three
/// ints, seven booleans, two more ints, a long. **The field order and what
/// each one means is recalled from WebRTC's own public
/// `WebRtcAudioManager.java` (`storeAudioParameters()`), not read out of
/// Roblox's bytecode body, and is `INFERRED`** — but the recollection is
/// corroborated rather than assumed: laid out as sample rate, output
/// channels, input channels, hardware AEC, hardware AGC, hardware NS, low
/// latency output, low latency input, pro audio, AAudio, output buffer size,
/// input buffer size, native handle, it reproduces `IIIZZZZZZZIIJ` exactly,
/// which a wrong guess at the split between the seven booleans and the
/// trailing pair of ints would not do by chance.
///
/// Every value passed is one this file already answers elsewhere in this
/// class, so there is nothing here that could disagree with a getter a
/// caller might check instead: mono both ways
/// (`getStereoInput`/`getStereoOutput`), no hardware effects
/// (`isAcousticEchoCancelerSupported` and its neighbours), no low-latency
/// path (`isLowLatencyInputSupported`/`Output`), and a buffer size of
/// `getLowLatencyOutputFramesPerBuffer()`'s own 10 ms figure — which is what
/// `getMinOutputFrameSize` computes too once `sampleRate` is filled in, so
/// the two formulas agree here regardless of which one real WebRTC actually
/// evaluates.
///
/// Failure is a missing native, not a missing engine: if
/// `RegisterNatives` has not reached this class yet, this logs once and
/// returns `false` without inventing a call that has nowhere to land — the
/// engine's C++ side then runs without the parameters it would have cached,
/// which is a real gap and is reported as one, all the way out through
/// `WebRtcAudioManager::init`'s own return value now, rather than silently
/// skipped and then claimed as success.
bool cache_audio_parameters(ENV* env, Object* self, jlong native_audio_manager) {
    void* fn = find_registered_native(env, "org/webrtc/voiceengine/WebRtcAudioManager",
                                       "nativeCacheAudioParameters");
    if (!fn) {
        std::fprintf(stderr,
            "W/Cordial-Audio           WebRtcAudioManager.init: nativeCacheAudioParameters was "
            "never registered by the engine; its own audio device module will not have these "
            "figures cached, and init() is told so rather than reporting success anyway.\n");
        return false;
    }
    using CacheFn = void (*)(JNIEnv*, jobject, jint, jint, jint, jboolean, jboolean, jboolean,
                              jboolean, jboolean, jboolean, jboolean, jint, jint, jlong);
    JNIEnv* jni = env->GetJNIEnv();
    jobject thiz = (jobject)to_jni(env, self->shared_from_this());
    constexpr jint kBufferFrames = DEFAULT_SAMPLE_RATE / 100; // 10 ms, matching the getters above
    reinterpret_cast<CacheFn>(fn)(jni, thiz,
        DEFAULT_SAMPLE_RATE, // sampleRate
        1,                   // outputChannels (mono)
        1,                   // inputChannels (mono)
        JNI_FALSE,           // hardwareAEC
        JNI_FALSE,           // hardwareAGC
        JNI_FALSE,           // hardwareNS
        JNI_FALSE,           // lowLatencyOutput
        JNI_FALSE,           // lowLatencyInput
        JNI_FALSE,           // proAudio
        JNI_FALSE,           // aAudio
        kBufferFrames,       // outputBufferSize, in frames
        kBufferFrames,       // inputBufferSize, in frames
        native_audio_manager);
    return true;
}

// -------------------------------------- org.webrtc.voiceengine.WebRtcAudioRecord

/// `org.webrtc.voiceengine.WebRtcAudioRecord`
///
/// Roblox's actual microphone path, and therefore where the rule at the top of
/// this file has to hold in practice rather than in principle: everything
/// below opens `capture` from exactly one place (`startRecording`) and closes
/// it from every path that can end a recording — `stopRecording`,
/// `releaseAudioResources`, and the destructor, for the drop-without-stopping
/// case `WebRtcAudioTrack::stop` and `opensles.cpp`'s `recorder_Destroy` both
/// already guard against.
///
/// **Implemented now.** `initRecording` used to refuse unconditionally because
/// this class had not yet hooked its own `<init>(J)V` and so had no
/// `nativeAudioRecord` handle to hand back through `nativeDataIsRecorded` —
/// the same handle `WebRtcAudioTrack` threads through its own constructor for
/// `nativeGetPlayoutData`. `ctor` below closes that gap the same way.
///
/// **The pull loop is `opensles.cpp`'s `AudioRecorderObject::run_pump`,
/// applied to WebRTC's shape instead of OpenSL's, not a second design.** Both
/// exist for the same underlying fact: `CaptureStream::read` never blocks —
/// its own header says why, and it is the reason `AudioRecord::read` above
/// leaves the polling to whichever Java thread calls it — so whatever drives
/// this class has to supply its own wait. `opensles.cpp` polls at 2 ms because
/// that is a quarter of the shortest buffer either caller uses, WebRTC's own
/// 10 ms frame, and this class is that caller — it uses the same figure
/// rather than picking a second one that could drift out of step with it.
///
/// **What is verified against the shipping dex and what is recalled**, on the
/// same terms `WebRtcAudioTrack`'s own comment below sets out.
/// `initRecording(II)I`, `startRecording()Z`, `stopRecording()Z`,
/// `releaseAudioResources()V`, `nativeCacheDirectBufferAddress
/// (Ljava/nio/ByteBuffer;J)V` and `nativeDataIsRecorded(IJ)V` all came from
/// `tools/dex_method.py` against the shipping dex. That `initRecording`
/// allocates a direct buffer sized to one 10 ms frame and returns its
/// capacity in bytes, and that a dedicated thread stands in for WebRTC's own
/// `AudioRecordThread` reading into that buffer and calling
/// `nativeDataIsRecorded(bytesRead, nativeAudioRecord)` once per full frame,
/// is recalled from WebRTC's long-public `WebRtcAudioRecord.java` and is
/// `INFERRED` rather than read out of Roblox's bytecode body — Roblox's copy
/// of this class is not decompiled anywhere in this tree, only its declared
/// method table is.
///
/// **`setMicrophoneMute` stays exactly as it was: recorded, not acted on.**
/// Real WebRTC zeroes the delivered buffer while muted rather than closing
/// anything — a statement about *content*, which this file has no rule about,
/// not about the *stream*'s existence, which is what the microphone rule
/// above governs — but wiring that in is separate, small work this change
/// does not take on, and the flag was already honestly documented as unused
/// before this change touched anything else in this class.
///
/// **The one-way-door question `WebRtcAudioManager`'s own comment raises is
/// exactly as open as it was**: whether WebRTC's C++ audio device module
/// treats one half failing as fatal to the other has still not been observed
/// from a live call, only reasoned about. See that class's comment and the
/// report accompanying this change for what running signed-in sessions here
/// actually established versus what still needs the maintainer's rig.
class WebRtcAudioRecord : public Object {
public:
    audio::CaptureStream capture;

    /// The engine's own handle from `<init>(J)V`, handed back through
    /// `nativeDataIsRecorded` exactly as `WebRtcAudioTrack::nativeAudioTrack`
    /// is handed back through `nativeGetPlayoutData`.
    jlong nativeAudioRecord = 0;

    uint32_t sampleRate = DEFAULT_SAMPLE_RATE;
    uint32_t channels = 1;

    /// Same ownership rule as `WebRtcAudioTrack::playoutBuffer`: this is the
    /// address `nativeCacheDirectBufferAddress` hands the engine, and it must
    /// stay valid for exactly as long as this member does, independent of
    /// whatever happens to the `jobject` wrapper built around it.
    std::unique_ptr<uint8_t[]> recordBuffer;
    uint32_t bufferBytes = 0;

    std::thread pump;
    std::atomic<bool> recording{false};
    std::string targetNode;

    /// Process-wide, not per-instance, because `setMicrophoneMute` is static
    /// in the dex -- there is no receiver to hang it off. It was previously
    /// stored on the instance and set through a `dynamic_cast` of the receiver,
    /// which could never have fired: the hook was registered as an instance
    /// method and so never bound at all.
    static inline std::atomic<bool> microphoneMuted{false};

    static std::shared_ptr<WebRtcAudioRecord> ctor(ENV*, Class*, jlong native_audio_record) {
        auto r = std::make_shared<WebRtcAudioRecord>();
        r->nativeAudioRecord = native_audio_record;
        return r;
    }

    /// `initRecording(sampleRate, channels)`. Allocates the direct buffer and
    /// hands its address to the engine; opens no capture stream yet, matching
    /// every other object in this file that keeps construction and
    /// Realize-equivalents silent until something actually asks to run — the
    /// microphone rule would be worth nothing if this method already touched
    /// PipeWire ahead of `startRecording`.
    static jint initRecording(ENV* env, Object* self, jint sample_rate, jint num_channels) {
        auto* r = as(self);
        if (!r || sample_rate <= 0 || num_channels <= 0) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord.initRecording(%d Hz, %d ch): "
                "refusing an impossible format.\n", sample_rate, num_channels);
            return -1;
        }
        // Below this point, a live `recording` means `run_pump` may right now
        // be inside `capture.read(recordBuffer.get() + filled, ...)` on
        // another thread. Reallocating `recordBuffer` and handing the engine
        // a new address while that read is in flight is a use-after-free on
        // whichever address the pump thread still holds -- not a wrong
        // sample count, an actual dangling pointer -- so this refuses rather
        // than risking it. Real WebRTC does not call `initRecording` a second
        // time without an intervening `stopRecording`; there is nothing here
        // to migrate a running recording onto a new format, only a refusal.
        if (r->recording.load()) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord.initRecording(%d Hz, %d ch): called "
                "while already recording; reallocating recordBuffer now would free memory the "
                "pump thread may be reading, so refusing rather than risking a use-after-free.\n",
                sample_rate, num_channels);
            return -1;
        }
        void* cache_fn = find_registered_native(
            env, "org/webrtc/voiceengine/WebRtcAudioRecord", "nativeCacheDirectBufferAddress");
        if (!cache_fn) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord.initRecording: "
                "nativeCacheDirectBufferAddress was never registered by the engine, so there is "
                "nobody on the other end of a buffer this would hand over; refusing rather than "
                "opening a microphone nothing will ever read from.\n");
            return -1;
        }
        r->sampleRate = static_cast<uint32_t>(sample_rate);
        r->channels = static_cast<uint32_t>(num_channels);
        // 10 ms, WebRTC's own frame period and the same figure
        // `WebRtcAudioTrack::initPlayout` and `WebRtcAudioManager`'s
        // low-latency getters above already agree on.
        uint32_t frames = r->sampleRate / 100;
        if (frames == 0) frames = 1;
        r->bufferBytes = frames * r->channels * 2; // S16
        r->recordBuffer = std::make_unique<uint8_t[]>(r->bufferBytes);
        std::memset(r->recordBuffer.get(), 0, r->bufferBytes);

        JNIEnv* jni = env->GetJNIEnv();
        jobject jbuf = jni->NewDirectByteBuffer(r->recordBuffer.get(),
                                                 static_cast<jlong>(r->bufferBytes));
        jobject thiz = (jobject)to_jni(env, r->shared_from_this());
        using CacheFn = void (*)(JNIEnv*, jobject, jobject, jlong);
        reinterpret_cast<CacheFn>(cache_fn)(jni, thiz, jbuf, r->nativeAudioRecord);

        std::fprintf(stderr,
            "I/Cordial-Audio           WebRtcAudioRecord.initRecording(%d Hz, %d ch): %u byte "
            "buffer cached with the engine; no capture stream until startRecording.\n",
            sample_rate, num_channels, r->bufferBytes);
        return static_cast<jint>(r->bufferBytes);
    }

    /// **One of exactly two places in Cordial that open the microphone** —
    /// `AudioRecord::startRecording` above is the other, and
    /// `pipewire_backend.h`'s own comment on `CaptureStream::open` names both
    /// as the only permitted callers.
    static jboolean startRecording(ENV* env, Object* self) {
        auto* r = as(self);
        if (!r || !r->recordBuffer) {
            std::fprintf(stderr,
                "W/Cordial-Audio           WebRtcAudioRecord.startRecording called before a "
                "successful initRecording; refusing.\n");
            return false;
        }
        if (r->recording.load()) return true;
        void* data_fn = find_registered_native(
            env, "org/webrtc/voiceengine/WebRtcAudioRecord", "nativeDataIsRecorded");
        if (!data_fn) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord.startRecording: nativeDataIsRecorded "
                "was never registered by the engine; there is nowhere to deliver samples, so "
                "refusing rather than opening a microphone that would record into nothing.\n");
            return false;
        }
        // Read at the moment recording starts rather than cached, so
        // switching the desktop's default microphone between two voice
        // sessions is picked up without restarting the client — the same
        // reasoning `AudioRecord::startRecording` above already applies.
        r->targetNode = source_node_name_for_recording();
        if (!r->capture.open(r->sampleRate, r->channels, r->targetNode)) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord.startRecording could not open a "
                "capture stream; staying stopped rather than reporting a recording that is not "
                "happening.\n");
            return false;
        }
        r->recording.store(true);
        // Captures a `shared_ptr`, not `this`, for the identical reason
        // `WebRtcAudioTrack::startPlayout` does: the pump must not outlive the
        // object it reads `recordBuffer`/`nativeAudioRecord` off.
        auto self_ref = std::static_pointer_cast<WebRtcAudioRecord>(r->shared_from_this());
        r->pump = std::thread([self_ref, data_fn] { self_ref->run_pump(data_fn); });
        std::fprintf(stderr,
            "I/Cordial-Audio           WebRtcAudioRecord.startRecording: microphone opened at "
            "%u Hz, %u channel(s).\n", r->sampleRate, r->channels);
        return true;
    }

    static jboolean stopRecording(ENV*, Object* self) {
        if (auto* r = as(self)) r->stop();
        return true;
    }

    /// WebRTC calls this to release the underlying `AudioRecord` after a
    /// recording has stopped, or on an error path that never started one.
    /// `stop()` already handles both — it is idempotent whether or not a
    /// recording is in progress — so this needs no separate logic, matching
    /// `WebRtcAudioTrack::releaseAudioResources` immediately below in spirit.
    static void releaseAudioResources(ENV*, Object* self) {
        if (auto* r = as(self)) r->stop();
    }

    static void setMicrophoneMute(ENV*, Class*, jboolean mute) {
        microphoneMuted.store(mute);
    }

    // Both report that the hardware effect is unavailable, matching
    // WebRtcAudioManager above. WebRTC falls back to its own software AEC/NS,
    // which is the outcome that actually works here.
    static jboolean enableBuiltInAEC(ENV*, Object*, jboolean) { return false; }
    static jboolean enableBuiltInNS(ENV*, Object*, jboolean) { return false; }

    static jint getDefaultAudioSource(ENV*, Class*) {
        return 7; // MediaRecorder.AudioSource.VOICE_COMMUNICATION
    }

    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioRecord>("org/webrtc/voiceengine/WebRtcAudioRecord");
        auto c = env->GetClass("org/webrtc/voiceengine/WebRtcAudioRecord");
        c->Hook(env, "<init>", &WebRtcAudioRecord::ctor);
        c->HookInstanceFunction(env, "initRecording", &WebRtcAudioRecord::initRecording);
        c->HookInstanceFunction(env, "startRecording", &WebRtcAudioRecord::startRecording);
        c->HookInstanceFunction(env, "stopRecording", &WebRtcAudioRecord::stopRecording);
        c->HookInstanceFunction(env, "releaseAudioResources",
                                &WebRtcAudioRecord::releaseAudioResources);
        c->Hook(env, "setMicrophoneMute", &WebRtcAudioRecord::setMicrophoneMute);
        c->HookInstanceFunction(env, "enableBuiltInAEC", &WebRtcAudioRecord::enableBuiltInAEC);
        c->HookInstanceFunction(env, "enableBuiltInNS", &WebRtcAudioRecord::enableBuiltInNS);
        c->Hook(env, "getDefaultAudioSource", &WebRtcAudioRecord::getDefaultAudioSource);
    }

    /// Stops the pump and closes the capture stream. Idempotent — `stopRecording`
    /// and `releaseAudioResources` both call it, and so does the destructor, on
    /// the same reasoning `WebRtcAudioTrack::stop` and `opensles.cpp`'s
    /// `recorder_Destroy` both state: a dropped object must not leave the
    /// microphone open behind it, including when the engine drops it without
    /// stopping first.
    ///
    /// No self-join guard is needed here the way `opensles.cpp`'s
    /// `AudioRecorderObject::stop_capture` needs one for its own pump thread:
    /// that guard exists because OpenSL ES's buffer-queue calling convention
    /// explicitly permits calling `Destroy` from inside the very callback the
    /// pump thread invokes, so a naive `join()` there can be a thread joining
    /// itself. `nativeDataIsRecorded` is a data callback into WebRTC's audio
    /// processing module, not a call back into `stopRecording`, and stopping a
    /// real voice call happens from a different native thread — the same
    /// asymmetry `WebRtcAudioTrack::stop` already relies on for its own
    /// unconditional `pump.join()`.
    ///
    /// **Joins `pump` unconditionally, even down the "was already stopped"
    /// branch** — found while proving `run_pump`'s guard above closes the
    /// microphone: that guard can clear `recording` from the pump thread
    /// itself, on the attach-failure path, without this function ever
    /// running. A `stop()` that then took the fast branch on those grounds
    /// used to return without joining, and `std::thread::~thread()` calls
    /// `std::terminate()` on a still-joinable thread — so the destructor
    /// below would have crashed the process the first time that rare path
    /// was ever hit twice (attach fails, then the object is dropped with no
    /// explicit `stopRecording`/`releaseAudioResources` in between). Not one
    /// of the eight findings this change answers, but the same review that
    /// found the leak is what surfaces a fix left half-finished, so it is
    /// fixed alongside it rather than left for a tenth.
    void stop() {
        const bool was_recording = recording.exchange(false);
        if (pump.joinable()) pump.join();
        capture.close(); // idempotent; covers a stop before any start
        if (was_recording) {
            std::fprintf(stderr,
                "I/Cordial-Audio           WebRtcAudioRecord: microphone closed.\n");
        }
    }

    ~WebRtcAudioRecord() { stop(); }

private:
    static WebRtcAudioRecord* as(Object* o) { return dynamic_cast<WebRtcAudioRecord*>(o); }

    /// Stands in for WebRTC's own `AudioRecordThread.run()`. Runs until
    /// `stop()` clears `recording`.
    ///
    /// **Every exit from this function closes `capture`, structurally.**
    /// This used to be an early return's job on the attach-failure path below
    /// -- `recording.store(false); return;`, with no `capture.close()` beside
    /// it -- and it was reachable in review: `cordial::process_env()` failing
    /// left the object believing it was not recording while the PipeWire
    /// capture stream `startRecording` opened stayed open underneath it. That
    /// is exactly the state the microphone rule at the top of this file exists
    /// to prevent, and it does not need to be that rare a path to be a real
    /// bug: an early return that has to remember to close is the shape that
    /// produces this, and the shape recurred once already (`WebRtcAudioTrack`'s
    /// own copy, below). Closing in a guard's destructor removes the choice --
    /// there is no return statement anywhere in this function, present or
    /// future, that can skip it.
    void run_pump(void* data_fn) {
        // Closes `capture` and clears `recording` no matter which of this
        // function's exits runs, including the one that used to leak. When
        // `stop()` gets there first -- the ordinary path -- `recording` is
        // already false and `capture.close()` is a no-op; see its own header
        // for why that idempotence is guaranteed rather than assumed.
        struct PumpCloser {
            WebRtcAudioRecord* self;
            ~PumpCloser() {
                self->recording.store(false);
                self->capture.close();
            }
        } closer{this};

        // This thread did not exist when the process's `JavaVM` stood up, so
        // without attaching it here every JNI call below finds nothing —
        // `cordial::process_env()`'s own comment in `jni_shim.cpp` is the
        // established reason to reach for `AttachCurrentThread` rather than
        // `GetEnv`, exactly as `WebRtcAudioTrack::run_pump` already relies on
        // it for the identical problem on the playback side.
        auto* thread_env = cordial::process_env();
        if (!thread_env) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioRecord pump: could not attach to the "
                "process JavaVM; recording stops here.\n");
            return; // `closer` clears `recording` and closes `capture` here.
        }
        JNIEnv* jni = thread_env->GetJNIEnv();
        jobject thiz = (jobject)to_jni(thread_env, shared_from_this());
        using DataIsRecordedFn = void (*)(JNIEnv*, jobject, jint, jlong);
        auto data_is_recorded = reinterpret_cast<DataIsRecordedFn>(data_fn);

        uint32_t filled = 0;
        while (recording.load()) {
            uint32_t got = capture.read(recordBuffer.get() + filled, bufferBytes - filled);
            filled += got;
            if (filled < bufferBytes) {
                // `CaptureStream::read` never blocks -- its own header says
                // why -- so this loop supplies the wait that stands in for
                // `android.media.AudioRecord.read`'s blocking mode, at
                // `opensles.cpp`'s own 2 ms figure: a quarter of the 10 ms
                // frame this class and that one both poll for, chosen there
                // with this caller already in mind.
                std::this_thread::sleep_for(std::chrono::milliseconds(2));
                continue;
            }
            // The engine reads fresh samples straight out of `recordBuffer` --
            // the same address `initRecording` cached with it -- so nothing
            // beyond the byte count and the handle is passed back.
            data_is_recorded(jni, thiz, static_cast<jint>(bufferBytes), nativeAudioRecord);
            filled = 0;
        }
    }
};

// --------------------------------------- org.webrtc.voiceengine.WebRtcAudioTrack
//
// `org.webrtc.voiceengine.WebRtcAudioTrack`
//
// The voice-chat downlink: playing back what the other side of a call sent.
// Until this class existed, `WebRtcAudioManager::init()` refused specifically
// because this was missing — see that class's own comment — so this is the
// change that lets a signed-in client receive voice audio at all.
//
// **Which of Cordial's three audio paths carries voice chat was established
// by running, not assumed.** A signed-in `cordial-run` reaching Roblox's Home
// screen on 2.736.1408, `--dump-classes` against the real client, shows
// exactly `org/webrtc/voiceengine/{WebRtcAudioManager,WebRtcAudioRecord,
// WebRtcAudioTrack}` under `org/webrtc` — no `org/webrtc/audio/*` (the newer,
// non-deprecated `JavaAudioDeviceModule` some WebRTC builds use instead) and
// nothing under `com/roblox/audio` beyond the existing `AppRtcDeviceWrapper`.
// `docs/analysis/observed-java-surface.md` recorded the same three classes
// from an earlier, Landing-screen-only run, so this is not new information,
// only fresher: the class list has not changed shape across at least one
// build. AAudio's own input path (`native/aaudio.cpp`) and OpenSL ES's
// recorder (`native/opensles.cpp`) are both real, both tested, and both
// unrelated to voice specifically — their own headers say so, and neither
// showed a request from a signed-in session that never touched voice chat.
// **What was not run**: an actual voice call. Reaching one needs GUI
// navigation into a voice-enabled experience, which this change did not have
// a way to drive; see the report accompanying it for exactly what that
// leaves unverified.
//
// **`nativeCacheDirectBufferAddress` and `nativeGetPlayoutData` are native
// methods the *engine* provides, not ones Cordial answers.** Checked against
// `docs/analysis/jni-natives.tsv` — the same exported-symbol table every other
// JNI binding in this tree is checked against — and neither name appears
// there as a `Java_org_webrtc_voiceengine_WebRtcAudioTrack_native...` export.
// WebRTC's own C++ audio glue therefore binds them the way AGDK binds
// `GameActivity`'s lifecycle methods: dynamically, through `RegisterNatives`,
// which is why `find_registered_native` above exists and is used below
// exactly as `cordial_game_activity_start` uses its own copy of the same
// trick in `game_activity.cpp`.
//
// **The pull loop stands in for WebRTC's own `AudioTrackThread`.** Real
// Android has a dedicated Java thread that loops calling
// `nativeGetPlayoutData(sizeInBytes, nativeAudioTrack)` — which fills, in
// place, the direct `ByteBuffer` whose address `initPlayout` cached with
// `nativeCacheDirectBufferAddress` — and then blocks on
// `android.media.AudioTrack.write(..., WRITE_BLOCKING)`. There is no Java
// thread here, so `startPlayout` starts a C++ one that does the same two
// things: pull, then block. The block is `PlaybackStream::enqueue` paced by
// its drain callback, the identical mechanism `AudioDevice::write` above
// already uses and for the identical reason given there — return immediately
// instead and this thread pulls playout data as fast as the CPU allows,
// wildly ahead of anything PipeWire is actually draining, rather than at the
// rate audio is actually leaving the machine.
//
// **What is verified against the shipping dex and what is recalled.** Every
// method name and full descriptor below — `initPlayout(IID)I`,
// `nativeCacheDirectBufferAddress(Ljava/nio/ByteBuffer;J)V`,
// `nativeGetPlayoutData(IJ)V`, and the rest — came from `tools/dex_method.py`
// against the shipping dex, listed in the report accompanying this change.
// That a `WebRtcAudioTrack` object is normally driven by an `AudioTrackThread`
// pulling through a cached direct buffer, and that `initPlayout`'s three
// arguments are sample rate, channel count and a buffer-size multiplier, is
// recalled from WebRTC's own long-public `WebRtcAudioTrack.java` and is
// `INFERRED` rather than read out of Roblox's bytecode body — Roblox's copy
// of this class is not decompiled anywhere in this tree, only its declared
// method table is. The buffer-size arithmetic below is Cordial's own choice
// (WE are standing in for the Java method's entire implementation, so nothing
// downstream depends on reproducing its exact original formula), not a
// transcription of anything Roblox or WebRTC ships.
//
// **The one-way-door question is open, not answered.** `WebRtcAudioRecord`
// above still refuses `initRecording`. Whether the engine's own audio device
// module treats that as fatal to `WebRtcAudioTrack` too — the way a refused
// `AAudioStreamBuilder_openStream` cost FMOD every subsequent audio path, per
// `aaudio.cpp`'s header — has not been observed, because observing it needs a
// live voice call. If a real session shows `startPlayout` never being called
// at all, that is the first thing to check.
class WebRtcAudioTrack : public Object {
public:
    /// The engine's own handle from `<init>(J)V`, handed back on every native
    /// call below exactly as `WebRtcAudioManager::nativeAudioManager` is.
    jlong nativeAudioTrack = 0;

    uint32_t sampleRate = DEFAULT_SAMPLE_RATE;
    uint32_t channels = 1;

    /// The direct buffer's backing store. Owned here, not by the `jobject`
    /// `initPlayout` hands the engine — a jnivm `ByteBuffer` is a thin wrapper
    /// around whatever pointer it was built with
    /// (`third_party/libjnivm/include/jnivm/bytebuffer.h`), so this address
    /// stays valid for exactly as long as this member does, independent of
    /// what happens to that wrapper object afterwards.
    std::unique_ptr<uint8_t[]> playoutBuffer;
    uint32_t bufferBytes = 0;

    audio::PlaybackStream stream;
    std::thread pump;
    std::atomic<bool> playing{false};

    /// Guards `owned_` and `open_` only. `stream` itself is never touched
    /// while this is held — see `stop()`'s own comment for the deadlock that
    /// rule exists to prevent, the same one `AudioDevice::close` documents
    /// above for the identical reason.
    std::mutex lock_;
    std::condition_variable space_;
    bool open_ = false;
    std::vector<std::unique_ptr<uint8_t[]>> owned_;

    static std::shared_ptr<WebRtcAudioTrack> ctor(ENV*, Class*, jlong native_audio_track) {
        auto t = std::make_shared<WebRtcAudioTrack>();
        t->nativeAudioTrack = native_audio_track;
        return t;
    }

    /// `initPlayout(sampleRate, channels, bufferSizeFactor)`. Allocates the
    /// direct buffer and hands its address to the engine; opens nothing on
    /// PipeWire's side yet, matching every other object in this file that
    /// keeps construction and Realize-equivalents silent until something
    /// actually asks to run.
    static jint initPlayout(ENV* env, Object* self, jint sample_rate, jint num_channels,
                             jdouble buffer_size_factor) {
        auto* t = as(self);
        if (!t || sample_rate <= 0 || num_channels <= 0) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack.initPlayout(%d Hz, %d ch): refusing "
                "an impossible format.\n", sample_rate, num_channels);
            return -1;
        }
        void* cache_fn = find_registered_native(
            env, "org/webrtc/voiceengine/WebRtcAudioTrack", "nativeCacheDirectBufferAddress");
        if (!cache_fn) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack.initPlayout: "
                "nativeCacheDirectBufferAddress was never registered by the engine, so there is "
                "nobody on the other end of a buffer this would hand over; refusing rather than "
                "caching one nothing will ever fill.\n");
            return -1;
        }
        // The same hazard `WebRtcAudioRecord::initRecording` refuses: while
        // `playing`, the pump thread may be copying out of `playoutBuffer`, and
        // reallocating it here hands that thread -- and the engine, which has
        // the old address cached -- a freed buffer.
        if (t->playing.load()) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack.initPlayout(%d Hz, %d ch): called "
                "while already playing; reallocating playoutBuffer now would free memory the "
                "pump thread may be reading, so refusing.\n",
                sample_rate, num_channels);
            return -1;
        }
        t->sampleRate = static_cast<uint32_t>(sample_rate);
        t->channels = static_cast<uint32_t>(num_channels);
        const double factor = buffer_size_factor > 0.0 ? buffer_size_factor : 1.0;
        // 10 ms base period, matching `getLowLatencyOutputFramesPerBuffer` above
        // and `AAudioManager`'s own capture burst -- one buffer size this whole
        // file already agrees on, scaled by whatever margin the engine asked
        // for.
        uint32_t frames = static_cast<uint32_t>((sample_rate / 100.0) * factor);
        if (frames == 0) frames = 1;
        t->bufferBytes = frames * t->channels * 2; // S16
        t->playoutBuffer = std::make_unique<uint8_t[]>(t->bufferBytes);
        std::memset(t->playoutBuffer.get(), 0, t->bufferBytes);

        JNIEnv* jni = env->GetJNIEnv();
        jobject jbuf = jni->NewDirectByteBuffer(t->playoutBuffer.get(),
                                                 static_cast<jlong>(t->bufferBytes));
        jobject thiz = (jobject)to_jni(env, t->shared_from_this());
        using CacheFn = void (*)(JNIEnv*, jobject, jobject, jlong);
        reinterpret_cast<CacheFn>(cache_fn)(jni, thiz, jbuf, t->nativeAudioTrack);

        std::fprintf(stderr,
            "I/Cordial-Audio           WebRtcAudioTrack.initPlayout(%d Hz, %d ch, factor=%.2f): "
            "%u byte buffer cached with the engine.\n", sample_rate, num_channels, factor,
            t->bufferBytes);
        return static_cast<jint>(t->bufferBytes);
    }

    /// **One of the paths that opens PipeWire playback for a voice session.**
    /// Symmetric with `AudioRecorderObject::start_capture` in spirit even
    /// though this is output rather than input: nothing plays until the
    /// engine actually asks to start, and a failure here leaves nothing open
    /// behind it.
    static jboolean startPlayout(ENV* env, Object* self) {
        auto* t = as(self);
        if (!t || !t->playoutBuffer) {
            std::fprintf(stderr,
                "W/Cordial-Audio           WebRtcAudioTrack.startPlayout called before a "
                "successful initPlayout; refusing.\n");
            return false;
        }
        if (t->playing.load()) return true;
        void* pull_fn = find_registered_native(
            env, "org/webrtc/voiceengine/WebRtcAudioTrack", "nativeGetPlayoutData");
        if (!pull_fn) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack.startPlayout: nativeGetPlayoutData "
                "was never registered by the engine; there is nothing to pull data from, so "
                "refusing rather than opening a stream that would play silence forever.\n");
            return false;
        }
        if (!t->stream.open(t->sampleRate, t->channels, /*bits_per_sample=*/16,
                            /*container_bits=*/16, /*big_endian=*/false,
                            /*max_pending_buffers=*/4, audio::configured_output_device())) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack.startPlayout: PipeWire refused a "
                "%u Hz, %u channel S16 stream.\n", t->sampleRate, t->channels);
            return false;
        }
        t->stream.set_drain_callback(&WebRtcAudioTrack::drained, t);
        t->stream.set_active(true);
        {
            std::lock_guard<std::mutex> guard(t->lock_);
            t->open_ = true;
        }
        t->playing.store(true);
        // Captures a `shared_ptr`, not `this` -- the pump must not outlive the
        // object it reads `bufferBytes`/`nativeAudioTrack` off, and unlike
        // `AudioRecorderObject`'s raw-pointer pump in `opensles.cpp`, this
        // object's lifetime is ordinary `shared_ptr` refcounting rather than an
        // explicit `Destroy()` this file controls the timing of.
        auto self_ref = std::static_pointer_cast<WebRtcAudioTrack>(t->shared_from_this());
        t->pump = std::thread([self_ref, pull_fn] { self_ref->run_pump(pull_fn); });
        std::fprintf(stderr,
            "I/Cordial-Audio           WebRtcAudioTrack.startPlayout: pulling playout data at "
            "%u Hz, %u channel(s).\n", t->sampleRate, t->channels);
        return true;
    }

    static jboolean stopPlayout(ENV*, Object* self) {
        auto* t = as(self);
        if (t) t->stop();
        return true;
    }

    static void releaseAudioResources(ENV*, Object* self) {
        auto* t = as(self);
        if (t) t->stop();
    }

    static jint getBufferSizeInFrames(ENV*, Object* self) {
        auto* t = as(self);
        if (!t || t->channels == 0) return 0;
        return static_cast<jint>(t->bufferBytes / (t->channels * 2));
    }

    // Android's volume steps are 0..`getStreamMaxVolume()`, and the value
    // means nothing on its own -- only the ratio to the maximum does, which
    // is why `setStreamVolume` below normalises against the same constant
    // rather than passing the step count through unscaled.
    static constexpr jint kVolumeSteps = 15;
    static jint getStreamMaxVolume(ENV*, Object*) { return kVolumeSteps; }
    static jint getStreamVolume(ENV*, Object*) { return kVolumeSteps; }
    static jboolean setStreamVolume(ENV*, Object* self, jint volume) {
        auto* t = as(self);
        if (!t) return false;
        const jint clamped = volume < 0 ? 0 : (volume > kVolumeSteps ? kVolumeSteps : volume);
        t->stream.set_volume_linear(static_cast<float>(clamped) / kVolumeSteps);
        return true;
    }

    /// Static in the dex, not instance -- found by
    /// `tools/hook_descriptors.py`, exactly the check its own header says it
    /// exists to make: hooked as instance methods, both of these below
    /// registered cleanly and the engine would have called neither, ever,
    /// silently. Static because both are process-wide switches on real
    /// Android (one speakerphone, one audio-focus bucket per app, not one per
    /// `AudioTrack`), not because no instance exists yet when they are called.
    ///
    /// Recorded, not yet acted on, the same honest gap
    /// `WebRtcAudioRecord::microphoneMuted` above already documents for its
    /// own static mute flag: there is currently at most one live
    /// `WebRtcAudioTrack`, but nothing here tracks *which* one to reach from a
    /// static context, so wiring this to `PlaybackStream::set_mute` is left
    /// for whoever needs it rather than guessed at now.
    static inline std::atomic<bool> speakerMuted{false};
    static void setSpeakerMute(ENV*, Class*, jboolean mute) { speakerMuted.store(mute); }

    /// Android's `AudioAttributes.USAGE_*`, which routes a stream between the
    /// call and media audio-focus buckets in Android's own policy layer.
    /// PipeWire has no equivalent stream property exposed through
    /// `PlaybackStream`, so this is recorded nowhere and acted on nowhere --
    /// an honest no-op rather than a claim this file cannot back up.
    static void setAudioTrackUsageAttribute(ENV*, Class*, jint /*usage*/) {}

    static void Register(ENV* env) {
        env->GetClass<WebRtcAudioTrack>("org/webrtc/voiceengine/WebRtcAudioTrack");
        auto c = env->GetClass("org/webrtc/voiceengine/WebRtcAudioTrack");
        c->Hook(env, "<init>", &WebRtcAudioTrack::ctor);
        c->HookInstanceFunction(env, "initPlayout", &WebRtcAudioTrack::initPlayout);
        c->HookInstanceFunction(env, "startPlayout", &WebRtcAudioTrack::startPlayout);
        c->HookInstanceFunction(env, "stopPlayout", &WebRtcAudioTrack::stopPlayout);
        c->HookInstanceFunction(env, "releaseAudioResources",
                                &WebRtcAudioTrack::releaseAudioResources);
        c->HookInstanceFunction(env, "getBufferSizeInFrames",
                                &WebRtcAudioTrack::getBufferSizeInFrames);
        c->HookInstanceFunction(env, "getStreamMaxVolume", &WebRtcAudioTrack::getStreamMaxVolume);
        c->HookInstanceFunction(env, "getStreamVolume", &WebRtcAudioTrack::getStreamVolume);
        c->HookInstanceFunction(env, "setStreamVolume", &WebRtcAudioTrack::setStreamVolume);
        c->Hook(env, "setSpeakerMute", &WebRtcAudioTrack::setSpeakerMute);
        c->Hook(env, "setAudioTrackUsageAttribute", &WebRtcAudioTrack::setAudioTrackUsageAttribute);
    }

    /// Stops the pump and tears the stream down. Idempotent — `stopPlayout`
    /// and `releaseAudioResources` both call it, and so does the destructor,
    /// on the same reasoning `recorder_Destroy` states in `opensles.cpp`: a
    /// dropped object must not leave anything playing behind it, including
    /// when the engine drops it without stopping first.
    ///
    /// **Never call into `stream` while holding `lock_`.** `set_active`,
    /// `clear` and `close` each take PipeWire's own thread-loop lock, and
    /// `drained` -- called from that same thread -- takes `lock_`. Holding
    /// `lock_` across any of the three is the identical AB-BA that froze the
    /// client on 2026-08-22 for `AudioDevice::close`; see that method's own
    /// comment for the captured stacks. `enqueue`, in `run_pump` below, is
    /// the one `stream` method this rule does not cover, because
    /// `PlaybackStream::enqueue`'s own contract is that it never blocks and
    /// never takes that lock -- `AudioDevice::write` already relies on the
    /// same fact to call it from inside its own equivalent of `lock_`.
    ///
    /// **Joins `pump` unconditionally, even when `open_` was already
    /// false** — the same fix as `WebRtcAudioRecord::stop`'s own copy, found
    /// for the identical reason: `run_pump`'s own `PumpCloser` can clear
    /// `open_` from the pump thread itself, on the attach-failure path,
    /// without this function running at all. Returning early without joining
    /// used to leave `pump` joinable, and `std::thread::~thread()` calls
    /// `std::terminate()` on a still-joinable thread when this object is
    /// destroyed — a crash on top of whatever the guard already closed.
    void stop() {
        bool was_open;
        {
            std::lock_guard<std::mutex> guard(lock_);
            was_open = open_;
            open_ = false;
            playing.store(false);
            space_.notify_all();
        }
        if (pump.joinable()) pump.join();
        if (!was_open) return; // nothing left to close: never opened, or the guard got here first.
        stream.set_active(false);
        stream.clear();
        stream.close();
        {
            std::lock_guard<std::mutex> guard(lock_);
            owned_.clear();
        }
        std::fprintf(stderr, "I/Cordial-Audio           WebRtcAudioTrack: playout stream closed.\n");
    }

    ~WebRtcAudioTrack() { stop(); }

private:
    static WebRtcAudioTrack* as(Object* o) { return dynamic_cast<WebRtcAudioTrack*>(o); }

    /// Stands in for WebRTC's own `AudioTrackThread.run()`. Runs until `stop()`
    /// clears `open_`/`playing`.
    ///
    /// **Every exit from this function closes `stream`, structurally**, for
    /// the same reason and against the same bug class as
    /// `WebRtcAudioRecord::run_pump` above: the attach-failure return below
    /// used to clear `playing` and stop there, leaving the `PlaybackStream`
    /// `startPlayout` opened still connected. Playback leaking is a smaller
    /// harm than the microphone rule this file opens with -- there is no
    /// desktop indicator for an open playback stream the way there is for
    /// capture -- but it is the identical shape of bug (an early return that
    /// had to remember to close, and did not), on the class this file already
    /// names as `WebRtcAudioRecord`'s structural twin, so it gets the same
    /// fix rather than a smaller one.
    void run_pump(void* pull_fn) {
        // Mirrors `stop()`'s own cleanup exactly -- same order, same "never
        // call into `stream` while holding `lock_`" rule -- so that whichever
        // of the two reaches `open_` first does the real work and the other
        // finds it already false and does nothing. Ordinary shutdown still
        // goes through `stop()`: it clears `open_`/`playing` and joins this
        // thread, so by the time this destructor runs on that path `open_` is
        // already false and this is a no-op, exactly like
        // `WebRtcAudioRecord`'s `capture.close()` being idempotent above.
        struct PumpCloser {
            WebRtcAudioTrack* self;
            ~PumpCloser() {
                {
                    std::lock_guard<std::mutex> guard(self->lock_);
                    if (!self->open_) return; // stop() got here first; already closed.
                    self->open_ = false;
                    self->playing.store(false);
                }
                self->stream.set_active(false);
                self->stream.clear();
                self->stream.close();
                {
                    std::lock_guard<std::mutex> guard(self->lock_);
                    self->owned_.clear();
                }
                std::fprintf(stderr,
                    "I/Cordial-Audio           WebRtcAudioTrack: playout stream closed (pump "
                    "exited without stop() -- see run_pump's own comment).\n");
            }
        } closer{this};

        // This thread did not exist when the process's `JavaVM` stood up, so
        // without attaching it here every JNI call below finds nothing --
        // `cordial::process_env()`'s own comment in `jni_shim.cpp` is the
        // established reason to reach for `AttachCurrentThread` rather than
        // `GetEnv` for exactly this situation.
        auto* thread_env = cordial::process_env();
        if (!thread_env) {
            std::fprintf(stderr,
                "E/Cordial-Audio           WebRtcAudioTrack pump: could not attach to the "
                "process JavaVM; playout stops here.\n");
            return; // `closer` clears `open_`/`playing` and closes `stream` here.
        }
        JNIEnv* jni = thread_env->GetJNIEnv();
        jobject thiz = (jobject)to_jni(thread_env, shared_from_this());
        using GetDataFn = void (*)(JNIEnv*, jobject, jint, jlong);
        auto get_data = reinterpret_cast<GetDataFn>(pull_fn);
        static std::atomic<uint64_t> dropped{0};

        while (playing.load()) {
            // The engine writes fresh samples straight into `playoutBuffer` --
            // the same address `initPlayout` cached with it -- so there is
            // nothing to pass back beyond asking for the next block.
            get_data(jni, thiz, static_cast<jint>(bufferBytes), nativeAudioTrack);

            std::unique_lock<std::mutex> guard(lock_);
            if (!open_) return;
            auto owned = std::make_unique<uint8_t[]>(bufferBytes);
            std::memcpy(owned.get(), playoutBuffer.get(), bufferBytes);
            uint8_t* raw = owned.get();

            // Blocking, on purpose, for the reason `AudioDevice::write` above
            // blocks: this is the pacing for the whole path, standing in for
            // `android.media.AudioTrack.write`'s own blocking mode, which real
            // WebRTC's `AudioTrackThread` paces itself against. Spin instead
            // and this thread pulls playout data as fast as the CPU allows.
            const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(500);
            bool queued = stream.enqueue(raw, bufferBytes, raw);
            while (!queued) {
                if (!open_) return;
                if (space_.wait_until(guard, deadline) == std::cv_status::timeout) break;
                queued = stream.enqueue(raw, bufferBytes, raw);
            }
            if (queued) {
                owned_.emplace_back(std::move(owned));
            } else {
                const uint64_t n = ++dropped;
                if ((n & (n - 1)) == 0) {
                    std::fprintf(stderr,
                        "W/Cordial-Audio           WebRtcAudioTrack pump: no room after 500 ms, "
                        "dropped %llu buffer(s) so far -- the playback stream is not draining.\n",
                        static_cast<unsigned long long>(n));
                }
            }
        }
    }

    /// Called from PipeWire's own thread once a previously enqueued buffer has
    /// drained. Identical in shape to `AudioDevice::drained` above, for the
    /// same reason: `owned_` and the erase-by-pointer match are the same
    /// pattern, just applied to playout data pulled from the engine instead of
    /// pushed by FMOD.
    static void drained(void* buffer_context, void* user) {
        auto* self = static_cast<WebRtcAudioTrack*>(user);
        std::lock_guard<std::mutex> guard(self->lock_);
        for (auto it = self->owned_.begin(); it != self->owned_.end(); ++it) {
            if (it->get() == buffer_context) { self->owned_.erase(it); break; }
        }
        self->space_.notify_one();
    }
};

} // namespace

/// `CORDIAL_AUDIO_SELFTEST=1` prints the device list Roblox would be given, at
/// the moment the audio classes are registered.
///
/// It exists because of a gap that took a while to notice: the whole audio
/// backend can be verified out of process (`native/audio_probe.cpp` does), and
/// none of that says whether it works *inside* `cordial-run`, where bionic's
/// linker, libjnivm and a second libc are in the same address space. Until
/// something in Roblox actually asks for audio, this is the only way to find
/// out, and "nothing asked, so nothing was tested" is how a backend stays
/// broken for a year without anyone being wrong about it.
///
/// Off by default and enumeration-only. It opens no stream of any kind — see
/// the microphone rule at the top of this file, which this must not be the
/// exception to.
void audio_selftest() {
    if (!std::getenv("CORDIAL_AUDIO_SELFTEST")) return;
    std::vector<audio::DeviceInfo> devices = audio::enumerate_devices();
    std::fprintf(stderr,
        "I/Cordial-Audio           selftest: PipeWire reports %zu audio device(s).\n",
        devices.size());
    for (const audio::DeviceInfo& d : devices) {
        auto info = make_device_info(d);
        std::fprintf(stderr,
            "I/Cordial-Audio           selftest:   id=%d %s%s type=%d productName='%s'\n",
            info->id, d.is_source ? "input " : "output", d.is_default ? " (host default)" : "",
            info->type, info->productName.c_str());
    }
    std::fprintf(stderr,
        "I/Cordial-Audio           selftest: capture streams open = %u (must be 0: listing "
        "microphones does not use one).\n", audio::active_capture_streams());
}


// ------------------------------------------------------------- org.fmod.*

/// FMOD's Java audio path, which is the one Roblox actually takes.
///
/// This exists because of a measurement that overturned a long-held
/// assumption. `native/opensles.cpp` implements a complete OpenSL ES object
/// model over PipeWire, and it was believed to be Roblox's audio output. It is
/// not: on a signed-in run that reaches an experience, `slCreateEngine` is
/// called **zero** times, no audio library is `dlopen`ed, and FMOD fails at
/// `System::init` with `FMOD_RESULT 51` (`FMOD_ERR_OUTPUT_INIT`) before it ever
/// enumerates a device. What the engine does ask for, from the JNI trace:
///
///     FindClass org/fmod/AudioDevice
///     FindClass org/fmod/MediaCodec
///     FindClass org/fmod/FMOD
///     Unresolved org/fmod/FMOD        static checkInit          ()Z
///     Unresolved org/fmod/FMOD        static supportsLowLatency ()Z
///     Unresolved org/fmod/FMOD        static supportsAAudio     ()Z
///     Unresolved org/fmod/AudioDevice        <init>             ()V
///     Unresolved org/fmod/AudioDevice        init               (IIII)Z
///     Unresolved org/fmod/AudioDevice        write              ([BI)V
///     Unresolved org/fmod/AudioDevice        close              ()V
///
/// Six methods. That trace is the source for every signature here; none of it
/// is recalled from FMOD's SDK or read out of a decompilation.
///
/// The OpenSL ES model is deliberately left in place. It is complete, it is
/// tested, and a future Roblox build that prefers `SL_IID_ENGINE` would use it
/// — deleting it to celebrate this finding would throw away working code on the
/// strength of one build's preference.

/// `org.fmod.FMOD`
///
/// Three static predicates FMOD consults before choosing an output.
class FMOD : public jnivm::Object {
public:
    /// Whether FMOD's Java side is ready. True: the class is registered and
    /// `AudioDevice` below is real.
    static jboolean checkInit(jnivm::ENV*, jnivm::Class*) { return JNI_TRUE; }

    /// Android's low-latency fast-mixer path, which is an `AudioTrack` flag
    /// this class does not create. False rather than true because claiming it
    /// changes the buffer sizes FMOD asks for, and a stub that lies about
    /// latency produces underruns rather than an error anybody can trace.
    static jboolean supportsLowLatency(jnivm::ENV*, jnivm::Class*) { return JNI_FALSE; }

    /// **AAudio: true unless `CORDIAL_AUDIO=java` asked otherwise, or there is
    /// no PipeWire session to be true about.**
    ///
    /// This used to be an unconditional false, with a comment saying to flip
    /// it "the day `libaaudio.so` is real, not before". That day came in two
    /// halves. First `native/aaudio.cpp` implemented the 25 entry points this
    /// build looks up, over PipeWire, and `symtab.rs` registered them as a
    /// virtual `libaaudio.so` — behind `CORDIAL_AUDIO=aaudio`, because an
    /// audio backend nobody has measured must not become the default on an
    /// update. Then it was measured, capture was implemented, and it became
    /// the default; `CORDIAL_AUDIO=java` is the way back.
    ///
    /// Either way the answer is conditional on exactly the same reading of
    /// `CORDIAL_AUDIO` that decides whether the library exists at all. The two
    /// cannot disagree: both call `cordial_audio_backend_is_aaudio`, and there
    /// is one definition of it.
    ///
    /// **This predicate, not `dlopen`, is the real gate**, and that was worth
    /// measuring rather than assuming. `docs/analysis/aaudio-contract.md` had
    /// it that Roblox's `dlopen("libaaudio.so")` fails because Cordial
    /// registers no such library. It does not fail — it never happens. A
    /// signed-in run into place 1818 with `CORDIAL_TRACE_DLSYM=1` records six
    /// guest `dlopen` calls (`libc`, `libcamera2ndk`, `libmediandk`,
    /// `libvulkan`, `libandroid` twice) and no audio library among them:
    /// FMOD asks this Java method first and never looks for the library when
    /// the answer is false. Answer it true and `dlopen("libaaudio.so", 1)`
    /// appears in the very next few trace lines, followed by a `dlsym` for
    /// each of the 25 names.
    ///
    /// **Saying yes here is a commitment, not a preference, and that is the
    /// most consequential thing measured on 2026-08-22.** A control run
    /// (`CORDIAL_AUDIO=aaudio-refuse`) answered this true and then reported
    /// `AAUDIO_ERROR_UNAVAILABLE` from every `openStream`. FMOD tried twice —
    /// a probe with no callbacks, then the real stream — and on the second
    /// refusal **gave up on audio entirely**. It did not fall back to
    /// `AudioDevice` below, and it did not fall back to OpenSL ES: there is
    /// no `AudioDevice.init` anywhere in the rest of that log, and the place
    /// loaded and played in silence. The "AAudio, then OpenSL ES, then Java"
    /// chain this work was planned around does not exist on this build once
    /// the first link has been claimed.
    ///
    /// That is why `host_backend_available()` is part of the condition: for
    /// the default backend it is `pipewire_available()`, the same call
    /// `slCreateEngine` gates on. A session that connects is kept for the
    /// process; a failed one is retried at most every five seconds
    /// (`kSessionRetryAfter`), so a daemon that comes up late is found and a
    /// wedged one is not asked on every call. Without it a machine with no
    /// PipeWire daemon would have this method promise an output that
    /// `AAudioStreamBuilder_openStream` must then refuse — costing the Java
    /// fallback that would have reported the same failure in the one place
    /// people already know to look.
    static jboolean supportsAAudio(jnivm::ENV*, jnivm::Class*) {
        if (!cordial_audio_backend_is_aaudio()) return JNI_FALSE;
        if (!audio::host_backend_available()) {
            std::fprintf(stderr,
                "W/Cordial-FMOD            AAudio is selected but no PipeWire session is "
                "reachable; answering supportsAAudio() false so FMOD keeps its "
                "AudioDevice fallback. Claiming AAudio here would cost all audio, not just "
                "the low-latency path -- FMOD does not fall back once a stream it opened "
                "through AAudio refuses.\n");
            return JNI_FALSE;
        }
        return JNI_TRUE;
    }

    static void Register(jnivm::ENV* env) {
        env->GetClass<FMOD>("org/fmod/FMOD");
        auto c = env->GetClass("org/fmod/FMOD");
        c->Hook(env, "checkInit", &FMOD::checkInit);
        c->Hook(env, "supportsLowLatency", &FMOD::supportsLowLatency);
        c->Hook(env, "supportsAAudio", &FMOD::supportsAAudio);
    }
};

/// `org.fmod.AudioDevice` — FMOD's PCM sink, over `PlaybackStream`.
///
/// **`write` must copy.** `PlaybackStream::enqueue` is deliberately zero-copy:
/// the caller's buffer has to stay valid and unmodified until the drain
/// callback fires, which is the `SLAndroidSimpleBufferQueueItf` contract it was
/// built against. A `jbyteArray` does not satisfy that — the pointer is the
/// engine's array for the duration of this call and no longer. So each write
/// takes a copy this class owns and frees when PipeWire reports the buffer
/// drained. Handing the array pointer straight through would work for as long
/// as it took the realtime thread to fall one cycle behind, and then produce
/// audio built out of whatever the engine put there next.
class AudioDevice : public jnivm::Object {
public:
    /// `init(channels, sampleRate, bufferFrames, numBuffers)`.
    ///
    /// **Measured, not recalled.** The first version of this read the four
    /// arguments by magnitude and logged them, because the JNI trace gives the
    /// signature `(IIII)Z` and no names, and FMOD's own header is not a source
    /// this project reads. Roblox's first call was:
    ///
    ///     AudioDevice.init(2, 48000, 512, 4)
    ///
    /// which fixes the order beyond reasonable doubt — 48000 is the only
    /// argument that can be a rate, 2 the only sensible channel count, and 512
    /// frames x 4 buffers is an ordinary Android mixer configuration. Named
    /// parameters now, with the magnitude check kept as an assertion rather
    /// than as the mechanism: if a future build passes them in another order,
    /// this refuses instead of opening a stream at 2 Hz.
    jboolean init(jnivm::ENV*, jint channels, jint rate, jint buffer_frames, jint buffer_count) {
        std::fprintf(stderr,
            "I/Cordial-FMOD            AudioDevice.init(channels=%d, rate=%d, frames=%d, "
            "buffers=%d)\n", channels, rate, buffer_frames, buffer_count);

        if (rate < 8000 || rate > 192000 || channels < 1 || channels > 8) {
            std::fprintf(stderr,
                "E/Cordial-FMOD            AudioDevice.init: refusing (channels=%d, rate=%d). "
                "The argument order measured on 2026-08-06 was (channels, rate, frames, "
                "buffers); this build appears to pass something else. Reporting failure rather "
                "than opening a stream at a guessed format.\n", channels, rate);
            return JNI_FALSE;
        }

        // FMOD's own buffer count, not a number of ours. It sized its mixer
        // for this depth; a shallower queue drops what it legitimately wrote
        // and a deeper one adds latency it did not ask for.
        //
        // Computed into a local first because the three `stream_` calls below
        // must not run under `lock_` -- see `close`'s doc for the deadlock that
        // rule exists to prevent. `init` is the narrower case of it: a
        // re-`init` over a stream that is still live would have
        // `stream_.open()` take PipeWire's loop lock while `drained` waits on
        // ours. That path was never observed hanging, which is exactly why it
        // is worth closing now rather than after somebody catches it too.
        const uint32_t depth = buffer_count > 0 ? static_cast<uint32_t>(buffer_count) : 4;

        // A re-`init` over a live stream goes through `close` first, so the
        // old stream's teardown follows the ordering `close` documents and its
        // `owned_` buffers are not dropped while it can still drain them.
        close(nullptr);

        // S16 interleaved: what `write([BI)V` hands over as bytes, and what
        // every Android AudioTrack path FMOD has produces.
        //
        // The sink comes from `configured_output_device()` rather than from
        // anything FMOD said, and it cannot come from anywhere else: Roblox's
        // own picker is populated by FMOD's output backend, which presents a
        // single device on every path Cordial provides, and the AAudio path
        // has no `AAudioStreamBuilder_setDeviceId` for the engine to ask with
        // (`docs/analysis/aaudio-contract.md`). Empty is the default and means
        // the session's own default sink, followed live.
        if (!stream_.open(static_cast<uint32_t>(rate), static_cast<uint32_t>(channels),
                          16, 16, false, depth, audio::configured_output_device())) {
            std::fprintf(stderr,
                "E/Cordial-FMOD            AudioDevice.init: PipeWire refused a %d Hz, %d "
                "channel S16 stream; reporting failure to FMOD.\n", rate, channels);
            return JNI_FALSE;
        }
        // Registered before the stream is activated, so there is no window in
        // which PipeWire can process a buffer with no callback to report it.
        stream_.set_drain_callback(&AudioDevice::drained, this);
        stream_.set_active(true);

        // Taken only once the stream is up. `write` and `drained` both gate on
        // `open_`, so publishing it last is what makes them safe rather than
        // merely serialised.
        std::lock_guard<std::mutex> guard(lock_);
        pending_depth_ = depth;
        open_ = true;
        std::fprintf(stderr,
            "I/Cordial-FMOD            AudioDevice.init: PipeWire playback open at %d Hz, %d "
            "channel(s), S16.\n", rate, channels);
        return JNI_TRUE;
    }

    void write(jnivm::ENV* env, std::shared_ptr<jnivm::Array<jbyte>> pcm, jint length) {
        (void)env;
        std::unique_lock<std::mutex> guard(lock_);
        if (!open_ || !pcm || length <= 0) return;
        const jint have = static_cast<jint>(pcm->getSize());
        const jint n = length < have ? length : have;
        auto owned = std::make_unique<uint8_t[]>(static_cast<size_t>(n));
        std::memcpy(owned.get(), pcm->getArray(), static_cast<size_t>(n));
        uint8_t* raw = owned.get();
        // **This blocks, and that is the whole point.**
        //
        // The first version returned immediately when the queue was full and
        // dropped the buffer. It produced audio, and the audio was terrible:
        // dropped buffer counts doubling 1, 2, 4, 8, 16, 32 within seconds of
        // a join. The reason is that `android.media.AudioTrack.write` — the
        // method FMOD believes it is calling — is *blocking* in its default
        // mode, and FMOD's mixer thread uses that block as its clock. Return
        // straight away and nothing paces the mixer: it produces as fast as the
        // CPU allows, overruns a queue sized for realtime, and every buffer
        // past the end is thrown away. What comes out is a fraction of the
        // audio, which is exactly what it sounds like.
        //
        // So wait for room, the way the method being imitated does. The wait is
        // on the same mutex the drain callback takes, which is what makes it
        // safe: waiting releases the lock, PipeWire's realtime thread drains a
        // buffer and notifies, and this wakes with room.
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(500);
        while (!stream_.enqueue(raw, static_cast<uint32_t>(n), raw)) {
            if (space_.wait_until(guard, deadline) == std::cv_status::timeout) {
                // Half a second without the stream taking a buffer is a stalled
                // device, not backpressure. Dropping beats blocking the engine's
                // mixer thread for ever, and it is reported rather than silent.
                static std::atomic<uint64_t> dropped{0};
                const uint64_t n_dropped = ++dropped;
                if ((n_dropped & (n_dropped - 1)) == 0) {
                    std::fprintf(stderr,
                        "W/Cordial-FMOD            AudioDevice.write: no room after 500 ms, "
                        "dropped %llu buffer(s) so far — the playback stream is not draining.\n",
                        static_cast<unsigned long long>(n_dropped));
                }
                return;
            }
            if (!open_) return;
        }
        owned_.emplace_back(std::move(owned));
    }

    /// **Never call into `stream_` while holding `lock_`.** That is the whole
    /// of this function's shape, and it is not a style preference -- it is the
    /// fix for a deadlock that froze the client whenever a player left a game.
    ///
    /// Observed on 2026-08-22, with gdb on a live specimen the user caught by
    /// exiting a game and reporting the window had stopped. Two threads, each
    /// holding what the other wanted:
    ///
    ///   RBX Worker B   AudioDevice::close  -> holds lock_
    ///                  PlaybackStream::set_active -> pw_thread_loop_lock, waits
    ///   cordial-pipewire  loop_iterate -> Impl::process -> holds the loop lock
    ///                  AudioDevice::drained -> lock_, waits
    ///
    /// Every `stream_` entry point takes PipeWire's thread-loop lock, and the
    /// loop thread calls `drained`, which takes `lock_`. So holding `lock_`
    /// across any `stream_` call is an AB-BA against PipeWire's own loop, and
    /// it never recovers.
    ///
    /// It cost two days pointed at the wrong component. Cordial's pump stays
    /// perfectly healthy through this -- `epoll_wait` in `looper::pump` at 1%
    /// CPU, 74 million polls on an earlier specimen -- because nothing is wrong
    /// with the pump. The engine's audio worker is blocked, so the engine stops
    /// feeding frames, and the client looks wedged from outside. Anyone
    /// debugging a freeze here should read the *engine's* threads before the
    /// looper's; see `docs/NEXT.md`.
    ///
    /// The race window is why it reproduced roughly one launch in twenty: the
    /// loop thread has to be inside `drained` at the moment `close` runs.
    void close(jnivm::ENV*) {
        {
            std::lock_guard<std::mutex> guard(lock_);
            if (!open_) return;
            // Cleared first, and under the lock, so a `write` blocked on
            // `space_` wakes, sees `!open_` and returns instead of enqueueing
            // into a stream that is about to be torn down. It also makes the
            // teardown below single-entry: a second `close` returns here.
            open_ = false;
            space_.notify_all();
        }

        // Deliberately outside the lock. `drained` may be running right now on
        // PipeWire's thread and may take `lock_`; it must be able to finish,
        // because these three calls each wait for that thread.
        stream_.set_active(false);
        stream_.clear();
        stream_.close();

        // Only now, with the stream closed and no further callback possible,
        // is it safe to drop the buffers `drained` erases from. Retaken rather
        // than held across the teardown, which is the entire point.
        {
            std::lock_guard<std::mutex> guard(lock_);
            owned_.clear();
        }
        std::fprintf(stderr, "I/Cordial-FMOD            AudioDevice.close: playback stream closed.\n");
    }

    static void Register(jnivm::ENV* env) {
        env->GetClass<AudioDevice>("org/fmod/AudioDevice");
        auto c = env->GetClass("org/fmod/AudioDevice");
        c->Hook(env, "<init>", [](jnivm::ENV* e, jnivm::Class* cl) {
            return std::make_shared<AudioDevice>(e, cl);
        });
        c->HookInstanceFunction(env, "init", &AudioDevice::init);
        c->HookInstanceFunction(env, "write", &AudioDevice::write);
        c->HookInstanceFunction(env, "close", &AudioDevice::close);
    }

    AudioDevice() = default;
    AudioDevice(jnivm::ENV*, jnivm::Class*) {}

private:
    /// Matches what `PlaybackStream::open` is told, so `enqueue` refuses at the
    /// same depth FMOD's own buffering expects rather than growing without end.
    uint32_t pending_depth_ = 4;

    /// Called from PipeWire's realtime thread, which holds that loop's own
    /// lock, and takes ours. **That direction is fixed, so no caller may ever
    /// hold `lock_` and then enter `stream_`** -- doing so is an AB-BA against
    /// PipeWire's loop and hangs the client for good. `close` used to, and
    /// froze the client every time a player left a game; its doc has the
    /// captured stacks.
    ///
    /// This comment previously read "with our own mutex released", stating the
    /// invariant as though it were guaranteed. It was true of `write` and false
    /// of `close`, and asserting it here is part of why the bug survived being
    /// read past.
    static void drained(void* buffer_context, void* user) {
        auto* self = static_cast<AudioDevice*>(user);
        std::lock_guard<std::mutex> guard(self->lock_);
        for (auto it = self->owned_.begin(); it != self->owned_.end(); ++it) {
            if (it->get() == buffer_context) { self->owned_.erase(it); break; }
        }
        // Wakes a `write` that is waiting for room. Notified with the lock
        // held, which is correct here: the waiter re-checks `enqueue` rather
        // than trusting a flag, so a spurious wake costs one retry.
        self->space_.notify_one();
    }

    std::mutex lock_;
    std::condition_variable space_;
    bool open_ = false;
    cordial::audio::PlaybackStream stream_;
    std::vector<std::unique_ptr<uint8_t[]>> owned_;
};

void register_audio_classes(jnivm::ENV* env) {
    CharSequence::Register(env);
    AudioDeviceInfo::Register(env);
    AudioManager::Register(env);
    AudioRecord::Register(env);
    WebRtcAudioManager::Register(env);
    WebRtcAudioRecord::Register(env);
    WebRtcAudioTrack::Register(env);
    FMOD::Register(env);
    AudioDevice::Register(env);
    audio_selftest();
}

} // namespace cordial
