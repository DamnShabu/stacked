// A copy of what Cordial plays, for the recorder (ADR-059, issue #33).
//
// Cordial owns every byte of sound the engine makes: AAudio's pull callback and
// OpenSL ES' buffer queue both end in a buffer this process fills and hands to a
// host backend. Recording "the game's sound" is therefore a matter of copying
// those buffers, not of asking the sound server for a mix that has other
// programs in it. The copy works the same over PipeWire, PulseAudio, ALSA and
// OSS because it is taken before the backend sees the data.
//
// **The audio thread never waits for the recorder.** `push` is a few atomic
// operations and a copy into a ring; a full ring drops the chunk and counts it.
// Nothing here locks, allocates, logs or makes a system call, which is the rule
// `CallbackStream::process` is written under (see `c7215eb` for what a mutex on
// that path did). With recording off, `push` is one relaxed load and a return.
//
// Header-only, with inline state, on purpose: `aaudio.cpp`, `opensles.cpp` and
// `pipewire_backend.cpp` are separate translation units, some of which are
// built into test executables that link none of the others. An inline variable
// is still one object per program, and it asks nothing of any link line.
//
// The reader is the recorder's pump thread (`cordial_record_tap_read`, in
// `pipewire_backend.cpp` next to the other C entry points). It always asks for
// 48000 Hz stereo float, so each source is converted on the way in (channels,
// sample format) and on the way out (rate). Sources are summed: FMOD may hold
// more than one stream open, and a recording that kept only the first would be
// missing whichever one played the music.
//
// **Sound is placed by when the engine produced it, and read late.** A backend
// hands the engine's buffers over in bursts (2048 frames every 43 ms was
// measured over PipeWire), so a reader that takes whatever has arrived the
// moment it asks runs the ring dry between bursts and punches holes in the
// sound: the first version did, 30% silence and a click every few hundred
// milliseconds, with every frame the engine produced sitting in the ring. So
// the first push after recording starts fixes where on the recording's timeline
// the stream begins (its steady-clock time, against the instant `enable` was
// called), later frames follow contiguously, and a push that arrives well after
// the end of the last one -- the stream was paused -- has the difference filled
// with silence. The pump reads positions on that timeline a fraction of a
// second behind the wall clock, so the frames it wants have always arrived.

#pragma once

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <memory>

namespace cordial::audio::tap {

/// What the recorder is given: always this, whatever the engine plays.
constexpr uint32_t kOutRate = 48000;

/// Frames a source's ring holds, a power of two. 32768 is 0.68 s at 48 kHz,
/// comfortably more than the reader's lag plus a backend's burst.
constexpr uint32_t kRingFrames = 1u << 15;
constexpr uint32_t kRingMask = kRingFrames - 1;

constexpr int kMaxSources = 8;

/// A push this much later than the end of the last one is a pause, and the gap
/// is filled with silence so what follows lands at the right time. Smaller
/// lateness is scheduling jitter and the sound simply continues.
constexpr int64_t kGapOut = kOutRate / 25; // 40 ms
/// A push this far ahead of the timeline means the sound card's clock is faster
/// than the wall clock; the surplus is dropped rather than left to become a
/// delay.
constexpr int64_t kAheadOut = kOutRate * 15 / 100; // 150 ms
constexpr int64_t kAheadKeepOut = kOutRate / 20;   // trimmed back to 50 ms

/// How much sample data in a chunk handed to `push` looks like.
enum class Fmt : uint8_t { None, S16, S32, F32 };

/// One playing stream's slot. Slots are never freed -- they live in a static
/// array and are reused -- so the reader can never be looking at memory that a
/// stream's close just released.
struct Source {
    /// Held by an open stream. Claimed with a compare-and-swap in `attach`.
    std::atomic<bool> claimed{false};
    /// The stream is open and its rate is valid for the reader.
    std::atomic<bool> live{false};
    std::atomic<uint32_t> rate{0};

    /// Frames written since `attach`, silence included; producer only.
    alignas(64) std::atomic<uint64_t> head{0};
    /// Frames the reader no longer needs, for the producer's room check.
    alignas(64) std::atomic<uint64_t> tail{0};
    std::atomic<uint64_t> dropped{0}; // frames refused by a full ring

    /// The timeline: frame `anchor_head` plays at output position `anchor_out`.
    /// Written by the producer, before `anchored` is released.
    std::atomic<bool> anchored{false};
    /// The `State::generation` the timeline belongs to. The reader ignores a
    /// timeline from an earlier recording even before its stream has pushed
    /// again to replace it.
    std::atomic<uint32_t> anchor_generation{0};
    std::atomic<int64_t> anchor_out{0};
    std::atomic<uint64_t> anchor_head{0};
    /// Producer only: the `State::generation` this slot last saw.
    uint32_t seen_generation = 0;

    float data[kRingFrames * 2]; // interleaved stereo
};

struct State {
    std::atomic<bool> enabled{false};
    /// Steady-clock nanoseconds at which output position 0 plays.
    std::atomic<int64_t> epoch_ns{0};
    /// Bumped by `enable(true)`; each source notices on its next push and starts
    /// its timeline afresh, so a recording never begins with earlier sound.
    std::atomic<uint32_t> generation{0};
    Source sources[kMaxSources];
};

inline State& state() {
    static State s;
    return s;
}

inline int64_t now_ns() {
    return std::chrono::duration_cast<std::chrono::nanoseconds>(
               std::chrono::steady_clock::now().time_since_epoch())
        .count();
}

inline bool enabled() { return state().enabled.load(std::memory_order_relaxed); }

/// Claim a slot for a stream that plays at `rate`. Not realtime: call it from
/// `open`. Null when every slot is taken, which `push` treats as "not tapped".
inline Source* attach(uint32_t rate) {
    if (rate == 0) return nullptr;
    for (Source& s : state().sources) {
        bool expected = false;
        if (!s.claimed.compare_exchange_strong(expected, true, std::memory_order_acq_rel)) continue;
        // Not live, so neither the reader nor a producer is touching it.
        s.head.store(0, std::memory_order_relaxed);
        s.tail.store(0, std::memory_order_relaxed);
        s.anchored.store(false, std::memory_order_relaxed);
        s.seen_generation = state().generation.load(std::memory_order_relaxed);
        s.rate.store(rate, std::memory_order_relaxed);
        s.live.store(true, std::memory_order_release);
        return &s;
    }
    return nullptr;
}

/// Give the slot back. Call it after the stream's audio thread has stopped, or
/// after pushes to it have been cut off by clearing the pointer they read.
inline void detach(Source* s) {
    if (!s) return;
    s->live.store(false, std::memory_order_release);
    s->anchored.store(false, std::memory_order_release);
    s->claimed.store(false, std::memory_order_release);
}

/// Start or stop wanting audio. Starting fixes the instant output position 0
/// plays -- now -- and makes every stream start its timeline again.
inline void enable(bool on) {
    State& st = state();
    if (on) {
        st.epoch_ns.store(now_ns(), std::memory_order_relaxed);
        st.generation.fetch_add(1, std::memory_order_acq_rel);
    }
    st.enabled.store(on, std::memory_order_release);
}

namespace detail {

inline float from_s16(const int16_t* p) { return static_cast<float>(*p) * (1.0f / 32768.0f); }
inline float from_s32(const int32_t* p) { return static_cast<float>(static_cast<double>(*p) * (1.0 / 2147483648.0)); }
inline float from_f32(const float* p) { return *p; }

template <class T, float (*Conv)(const T*)>
inline void put(Source& s, const uint8_t* bytes, uint32_t frames, uint32_t ch, uint64_t head) {
    const T* in = reinterpret_cast<const T*>(bytes);
    for (uint32_t i = 0; i < frames; ++i, in += ch) {
        float l, r;
        if (ch == 1) {
            l = r = Conv(in);
        } else if (ch >= 6) {
            // 5.1 / 7.1 as FL FR FC LFE then the surrounds. A plain fold-down,
            // not a decoder: a stereo file is what the recording is.
            const float c = 0.7071f * Conv(in + 2);
            const float sl = 0.7071f * Conv(in + 4);
            const float sr = 0.7071f * Conv(in + 5);
            l = Conv(in) + c + sl;
            r = Conv(in + 1) + c + sr;
            if (ch >= 8) {
                l += 0.7071f * Conv(in + 6);
                r += 0.7071f * Conv(in + 7);
            }
        } else {
            l = Conv(in);
            r = Conv(in + 1);
        }
        const size_t at = static_cast<size_t>((head + i) & kRingMask) * 2;
        s.data[at] = l;
        s.data[at + 1] = r;
    }
}

} // namespace detail

/// Copy `frames` frames of the stream's output into its ring, as of `at_ns`.
/// **Realtime.** `push` below is this with the clock read.
inline void push_at(Source* s, const void* pcm, uint32_t frames, uint32_t channels, Fmt fmt, int64_t at_ns) {
    if (!s || !pcm || frames == 0 || channels == 0 || fmt == Fmt::None) return;
    State& st = state();
    if (!st.enabled.load(std::memory_order_relaxed)) return;

    uint64_t head = s->head.load(std::memory_order_relaxed);
    const uint32_t gen = st.generation.load(std::memory_order_acquire);
    if (gen != s->seen_generation) {
        s->seen_generation = gen;
        s->anchored.store(false, std::memory_order_release);
        s->tail.store(head, std::memory_order_release);
    }
    const uint32_t rate = s->rate.load(std::memory_order_relaxed);
    const int64_t now_out =
        ((at_ns - st.epoch_ns.load(std::memory_order_relaxed)) * static_cast<int64_t>(kOutRate)) / 1000000000;

    const auto* bytes = static_cast<const uint8_t*>(pcm);
    uint32_t skip = 0; // leading frames of this chunk to drop
    uint64_t silence = 0;
    if (!s->anchored.load(std::memory_order_relaxed)) {
        s->anchor_out.store(now_out, std::memory_order_relaxed);
        s->anchor_head.store(head, std::memory_order_relaxed);
        s->anchor_generation.store(gen, std::memory_order_relaxed);
        s->anchored.store(true, std::memory_order_release);
    } else {
        const int64_t a_out = s->anchor_out.load(std::memory_order_relaxed);
        const uint64_t a_head = s->anchor_head.load(std::memory_order_relaxed);
        const int64_t expected_out =
            a_out + static_cast<int64_t>((head - a_head) * static_cast<uint64_t>(kOutRate) / rate);
        const int64_t late = now_out - expected_out;
        if (late > kGapOut) {
            const uint64_t want = static_cast<uint64_t>(late) * rate / kOutRate;
            if (want > kRingFrames / 4) {
                // Too long a pause to fill: start the timeline again here.
                s->anchor_out.store(now_out, std::memory_order_relaxed);
                s->anchor_head.store(head, std::memory_order_relaxed);
            } else {
                silence = want;
            }
        } else if (late < -kAheadOut) {
            const uint64_t surplus = static_cast<uint64_t>(-late - kAheadKeepOut) * rate / kOutRate;
            skip = surplus >= frames ? frames : static_cast<uint32_t>(surplus);
        }
    }

    const uint64_t tail = s->tail.load(std::memory_order_acquire);
    uint64_t room = kRingFrames - (head - tail);
    if (silence) {
        const uint64_t n = silence < room ? silence : room;
        for (uint64_t i = 0; i < n; ++i) {
            const size_t at = static_cast<size_t>((head + i) & kRingMask) * 2;
            s->data[at] = 0.0f;
            s->data[at + 1] = 0.0f;
        }
        head += n;
        room -= n;
    }
    const uint32_t stride = channels * (fmt == Fmt::S16 ? 2 : 4);
    bytes += static_cast<size_t>(skip) * stride;
    uint32_t n = frames - skip;
    if (n > room) {
        s->dropped.fetch_add(n - static_cast<uint32_t>(room), std::memory_order_relaxed);
        n = static_cast<uint32_t>(room);
    }
    if (n) {
        switch (fmt) {
        case Fmt::S16: detail::put<int16_t, detail::from_s16>(*s, bytes, n, channels, head); break;
        case Fmt::S32: detail::put<int32_t, detail::from_s32>(*s, bytes, n, channels, head); break;
        case Fmt::F32: detail::put<float, detail::from_f32>(*s, bytes, n, channels, head); break;
        case Fmt::None: break;
        }
        head += n;
    }
    s->head.store(head, std::memory_order_release);
}

inline void push(Source* s, const void* pcm, uint32_t frames, uint32_t channels, Fmt fmt) {
    if (!enabled()) return;
    push_at(s, pcm, frames, channels, fmt, now_ns());
}

/// The sample format `push` should be told about, from what a backend reports.
inline Fmt fmt_for(uint32_t bits, bool is_float) {
    if (is_float) return bits == 32 ? Fmt::F32 : Fmt::None;
    if (bits == 16) return Fmt::S16;
    if (bits == 32) return Fmt::S32;
    return Fmt::None;
}

/// Mix every live source into `dst` as 48 kHz stereo float: `frames` frames of
/// the recording's timeline starting at output position `pos0`.
///
/// Always fills the whole buffer: what a source has not supplied is silence,
/// which is what keeps the recording the right length when the game is quiet or
/// its stream is paused. **Reader thread only**, one reader at a time, and the
/// caller should ask for positions a little behind the wall clock (see the top
/// of this file).
inline void read_mix(float* dst, uint32_t frames, uint64_t pos0) {
    std::memset(dst, 0, static_cast<size_t>(frames) * 2 * sizeof(float));
    for (Source& s : state().sources) {
        if (!s.live.load(std::memory_order_acquire) || !s.anchored.load(std::memory_order_acquire)) continue;
        if (s.anchor_generation.load(std::memory_order_relaxed) != state().generation.load(std::memory_order_acquire)) continue;
        const uint32_t rate = s.rate.load(std::memory_order_relaxed);
        if (rate == 0) continue;
        const int64_t a_out = s.anchor_out.load(std::memory_order_relaxed);
        const uint64_t a_head = s.anchor_head.load(std::memory_order_relaxed);
        const uint64_t head = s.head.load(std::memory_order_acquire);
        const uint64_t have = head - a_head;
        const double step = static_cast<double>(rate) / kOutRate;
        const double f0 = static_cast<double>(static_cast<int64_t>(pos0) - a_out) * step;

        for (uint32_t i = 0; i < frames; ++i) {
            const double f = f0 + i * step;
            if (f < 0.0) continue;
            const uint64_t k = static_cast<uint64_t>(f);
            if (k + 1 >= have) break; // not arrived yet; the rest of this read is silence for it
            const uint64_t abs = a_head + k;
            if (head - abs >= kRingFrames - 1) continue; // already overwritten
            const float frac = static_cast<float>(f - static_cast<double>(k));
            const size_t a = static_cast<size_t>(abs & kRingMask) * 2;
            const size_t b = static_cast<size_t>((abs + 1) & kRingMask) * 2;
            dst[i * 2] += s.data[a] + (s.data[b] - s.data[a]) * frac;
            dst[i * 2 + 1] += s.data[a + 1] + (s.data[b + 1] - s.data[a + 1]) * frac;
        }
        // Everything before the next read's first frame is finished with.
        const double f_end = f0 + frames * step;
        if (f_end >= 1.0) {
            const uint64_t done = a_head + static_cast<uint64_t>(f_end) - 1;
            uint64_t t = s.tail.load(std::memory_order_relaxed);
            if (done > t && done <= head) s.tail.store(done, std::memory_order_release);
        }
    }
    for (uint32_t i = 0; i < frames * 2; ++i) {
        dst[i] = dst[i] > 1.0f ? 1.0f : (dst[i] < -1.0f ? -1.0f : dst[i]);
    }
}

} // namespace cordial::audio::tap
