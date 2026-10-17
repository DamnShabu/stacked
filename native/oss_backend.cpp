//! Open Sound System output, for the machines that have nothing else.
//!
//! ADR-023 scheduled OSS behind PulseAudio and ALSA and said, at the time
//! honestly, that it was "an answer to nothing" -- Linux has not shipped OSS as
//! its sound layer since ALSA replaced it, and the surviving users are on
//! FreeBSD, which Cordial does not run on. It was written up as the thing to
//! build if somebody arrived with the hardware and the appetite.
//!
//! Somebody asked. So it exists, and the reasoning that deferred it is left in
//! ADR-023 rather than quietly deleted, because "nobody needs this" turned out
//! to be a claim about who had spoken up rather than about who was there.
//!
//! ## It is the simplest backend here, and that is the point
//!
//! There is no client library to `dlopen`, no server to connect to, no node
//! graph and no registry. `/dev/dsp` is a file: three `ioctl`s to agree a
//! format, then `write()`. That means it works on a system with no sound
//! daemon at all, which is exactly the case the other three backends cannot
//! serve -- PipeWire and PulseAudio both need something running, and ALSA needs
//! a configured `default` PCM.
//!
//! It also means the device is **exclusive on most drivers**: while Cordial
//! holds `/dev/dsp`, nothing else on the machine gets sound. That is not a bug
//! to work around, it is what the interface is, and it is why this is opt-in
//! behind `CORDIAL_AUDIO_HOST=oss` and will not be selected for anybody who did
//! not ask.
//!
//! ## The write is the clock
//!
//! `write()` blocking when the device's buffer is full *is* the pacing, exactly
//! as `snd_pcm_writei` is for ALSA. There is no callback to wait for and no
//! sleep to tune, which is why the loop below has neither. Everything reachable
//! from `fill_` obeys `pipewire_backend.h`'s realtime rule -- no lock, no
//! allocation, no logging -- and the buffer is sized once in `open` for that
//! reason.
//!
//! ## Format
//!
//! OSS speaks integer PCM. `AFMT_S16_NE` is the one format every OSS
//! implementation worth targeting supports, so the engine's float frames are
//! converted on the way out rather than negotiated over. The conversion is a
//! multiply and a clamp per sample, on the audio thread, which is arithmetic
//! and allocates nothing.

#include "pipewire_backend.h"

#include <atomic>
#include <cstdio>
#include <cstring>
#include <thread>
#include <vector>

#if defined(CORDIAL_HAVE_OSS)

#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/soundcard.h>
#include <unistd.h>

namespace cordial::audio {
namespace {

/// What the engine is given, and what Cordial converts from.
constexpr uint32_t kChannels = 2;
constexpr uint32_t kRate = 48000;
/// Frames per write. About 10 ms at 48 kHz -- small enough that stopping is
/// responsive, large enough that the syscall rate is unremarkable.
constexpr uint32_t kPeriod = 480;

/// `CORDIAL_AUDIO_DEVICE`, or the conventional node.
const char* device_path() {
    const char* env = std::getenv("CORDIAL_AUDIO_DEVICE");
    if (env && env[0] != '\0') return env;
    return "/dev/dsp";
}

class OssStream final : public OutputStream {
public:
    OssStream() = default;
    ~OssStream() override { close(); }

    bool open(uint32_t sample_bits, bool is_float, const char* node_description,
              const char* target_node_name, FillCallback cb, void* user) override;
    void close() override;
    bool is_open() const override { return fd_ >= 0; }

    void set_running(bool running) override { running_.store(running, std::memory_order_relaxed); }
    bool is_running() const override { return running_.load(std::memory_order_relaxed); }

    uint32_t rate_hz() const override { return rate_; }
    uint32_t channels() const override { return channels_; }
    /// What the *engine* is handed, not what the device takes. The conversion
    /// to S16 happens on the way out; telling the engine 16-bit integer here
    /// would make it do the conversion instead, badly, and for no gain.
    uint32_t sample_bits() const override { return 32; }
    bool sample_is_float() const override { return true; }
    uint32_t burst_frames() const override { return period_; }
    uint64_t silence_cycles() const override { return silence_.load(std::memory_order_relaxed); }

private:
    void writer();

    int fd_ = -1;
    std::thread thread_;
    std::vector<float> buffer_;
    std::vector<int16_t> scratch_;

    FillCallback fill_ = nullptr;
    void* user_ = nullptr;
    uint32_t rate_ = kRate;
    uint32_t channels_ = kChannels;
    uint32_t period_ = kPeriod;

    std::atomic<bool> quit_{false};
    std::atomic<bool> running_{false};
    std::atomic<uint64_t> silence_{0};
};

void OssStream::writer() {
    const size_t samples = static_cast<size_t>(period_) * channels_;
    while (!quit_.load(std::memory_order_relaxed)) {
        if (running_.load(std::memory_order_relaxed) && fill_) {
            if (!fill_(buffer_.data(), period_, user_)) {
                // The engine asked to stop being pulled. Silence from here
                // rather than a teardown, for the reason `set_running` gives:
                // this may be reached from inside the engine's own callback and
                // closing the device underneath it would deadlock.
                running_.store(false, std::memory_order_relaxed);
            }
        } else {
            std::memset(buffer_.data(), 0, buffer_.size() * sizeof(float));
            silence_.fetch_add(1, std::memory_order_relaxed);
        }

        for (size_t i = 0; i < samples; ++i) {
            float v = buffer_[i];
            if (v > 1.0f) v = 1.0f;
            if (v < -1.0f) v = -1.0f;
            scratch_[i] = static_cast<int16_t>(v * 32767.0f);
        }

        const char* out = reinterpret_cast<const char*>(scratch_.data());
        size_t left = samples * sizeof(int16_t);
        while (left > 0) {
            ssize_t wrote = ::write(fd_, out, left);
            if (wrote < 0) {
                if (errno == EINTR) continue;
                // Unlike ALSA there is no recover() to call: an OSS write that
                // fails for anything but a signal has lost the device, and
                // retrying into it would spin. Reported once, then the stream
                // stops -- a gap that says so beats one that busy-loops.
                std::fprintf(stderr,
                    "E/Cordial-OSS             write to %s failed: %s; the stream is stopping.\n",
                    device_path(), std::strerror(errno));
                return;
            }
            out += wrote;
            left -= static_cast<size_t>(wrote);
        }
    }
}

bool OssStream::open(uint32_t, bool, const char*, const char*, FillCallback cb, void* user) {
    const char* path = device_path();
    // Opened non-blocking and then switched back: a blocking open of an OSS
    // device sleeps until its current holder closes it, which would wedge the
    // engine thread rather than report the device as busy.
    fd_ = ::open(path, O_WRONLY | O_CLOEXEC | O_NONBLOCK);
    if (fd_ >= 0) {
        int flags = ::fcntl(fd_, F_GETFL);
        if (flags < 0 || ::fcntl(fd_, F_SETFL, flags & ~O_NONBLOCK) < 0) {
            ::close(fd_);
            fd_ = -1;
        }
    }
    if (fd_ < 0) {
        std::fprintf(stderr, "W/Cordial-OSS             cannot open %s: %s\n", path,
                     std::strerror(errno));
        return false;
    }

    // **Order matters and is not arbitrary**: OSS requires format, then
    // channels, then rate. Setting the rate first and the format after lets a
    // driver reset the rate underneath you, which presents as audio at the
    // wrong pitch rather than as an error.
    int fmt = AFMT_S16_NE;
    if (::ioctl(fd_, SNDCTL_DSP_SETFMT, &fmt) < 0 || fmt != AFMT_S16_NE) {
        std::fprintf(stderr,
            "W/Cordial-OSS             %s will not take 16-bit native-endian PCM.\n", path);
        close();
        return false;
    }
    int ch = static_cast<int>(kChannels);
    if (::ioctl(fd_, SNDCTL_DSP_CHANNELS, &ch) < 0) {
        std::fprintf(stderr, "W/Cordial-OSS             %s refused %u channels.\n", path,
                     kChannels);
        close();
        return false;
    }
    int rate = static_cast<int>(kRate);
    if (::ioctl(fd_, SNDCTL_DSP_SPEED, &rate) < 0) {
        std::fprintf(stderr, "W/Cordial-OSS             %s refused %u Hz.\n", path, kRate);
        close();
        return false;
    }

    // **What came back, not what was asked for.** OSS negotiates by writing
    // through its argument, so a driver that only does 44100 says so here and
    // nowhere else. Reporting the requested rate instead is how a stream ends
    // up playing at the wrong speed with nothing in the log.
    channels_ = static_cast<uint32_t>(ch);
    rate_ = static_cast<uint32_t>(rate);
    period_ = kPeriod;

    buffer_.assign(static_cast<size_t>(period_) * channels_, 0.0f);
    scratch_.assign(static_cast<size_t>(period_) * channels_, 0);
    fill_ = cb;
    user_ = user;
    quit_.store(false, std::memory_order_relaxed);

    std::fprintf(stderr, "I/Cordial-OSS             %s at %u Hz, %u channels, %u-frame periods\n",
                 path, rate_, channels_, period_);
    thread_ = std::thread([this] { writer(); });
    return true;
}

void OssStream::close() {
    quit_.store(true, std::memory_order_relaxed);
    if (thread_.joinable()) thread_.join();
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
    fill_ = nullptr;
    user_ = nullptr;
    buffer_.clear();
    buffer_.shrink_to_fit();
    scratch_.clear();
    scratch_.shrink_to_fit();
}

} // namespace

bool oss_available() {
    // Opening the device is the only thing that proves it is there and free.
    // ADR-023 requires a probe this trustworthy because `supportsAAudio()` is a
    // one-way door: a backend that says yes and then cannot open leaves the
    // session silent with no fallback.
    //
    // **And on OSS the probe is more intrusive than elsewhere**, because the
    // device is usually exclusive: this open briefly takes sound away from
    // whatever holds it. Cached for that reason as much as for cost, and it is
    // another reason this backend is only reached when asked for by name.
    static const bool ok = [] {
        int fd = ::open(device_path(), O_WRONLY | O_CLOEXEC | O_NONBLOCK);
        if (fd < 0) return false;
        ::close(fd);
        return true;
    }();
    return ok;
}

std::unique_ptr<OutputStream> make_oss_stream() { return std::make_unique<OssStream>(); }

} // namespace cordial::audio

#else // !CORDIAL_HAVE_OSS

namespace cordial::audio {

// Built without <sys/soundcard.h>. Honest rather than absent, so the selector
// can tell a run that asked for OSS that it did not get it.
bool oss_available() { return false; }

std::unique_ptr<OutputStream> make_oss_stream() { return nullptr; }

} // namespace cordial::audio

#endif
