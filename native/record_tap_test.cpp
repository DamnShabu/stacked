// Checks the recorder's copy of the engine's sound (`record_tap.h`, ADR-059)
// without a sound server, a daemon or any way to make a noise: it feeds the ring
// the way a backend's audio thread would, with times of its own choosing, and
// reads it back the way the recorder's pump does.
//
// The properties that matter are the ones a wrong tap would get wrong silently:
// that sound pushed while nothing is recording is not kept, that two streams are
// summed rather than one winning, that what a stream has not supplied reads back
// as silence and not as stale samples, that a backend which delivers in bursts
// comes out without holes, and that a stream playing at a different rate comes
// out at the right length.

#include "record_tap.h"

#include <cassert>
#include <cmath>
#include <cstdio>
#include <vector>

namespace tap = cordial::audio::tap;

namespace {

constexpr float kPi = 3.14159265358979f;
constexpr int64_t kMs = 1000000;

/// Recording starts at time zero, so a push at `t` ms lands at position t * 48.
void start() {
    tap::enable(false);
    for (auto& s : tap::state().sources) {
        tap::detach(&s);
        s.dropped.store(0);
    }
    tap::enable(true);
    tap::state().epoch_ns.store(0);
}

std::vector<float> read(uint64_t pos, uint32_t frames) {
    std::vector<float> out(static_cast<size_t>(frames) * 2);
    tap::read_mix(out.data(), frames, pos);
    return out;
}

void push_f32(tap::Source* s, const std::vector<float>& stereo, int64_t at_ms) {
    tap::push_at(s, stereo.data(), static_cast<uint32_t>(stereo.size() / 2), 2, tap::Fmt::F32, at_ms * kMs);
}

float peak(const std::vector<float>& v) {
    float p = 0;
    for (float x : v) p = std::fmax(p, std::fabs(x));
    return p;
}

void nothing_is_kept_while_not_recording() {
    tap::enable(false);
    tap::Source* s = tap::attach(48000);
    assert(s);
    std::vector<float> in(960 * 2, 0.5f);
    tap::push(s, in.data(), 960, 2, tap::Fmt::F32);
    tap::enable(true);
    tap::state().epoch_ns.store(0);
    assert(peak(read(0, 480)) == 0.0f);
    tap::detach(s);
    std::printf("ok: nothing_is_kept_while_not_recording\n");
}

void sound_lands_at_the_position_it_was_pushed() {
    start();
    tap::Source* s = tap::attach(48000);
    // First push 100 ms in: before that is silence, from there it is sound.
    std::vector<float> in(480 * 2);
    for (int i = 0; i < 480; ++i) { in[i * 2] = 0.25f; in[i * 2 + 1] = -0.5f; }
    push_f32(s, in, 100);
    auto out = read(0, 6000);
    assert(out[4799 * 2] == 0.0f);                      // position 4799, a frame before
    assert(out[4800 * 2] == 0.25f && out[4800 * 2 + 1] == -0.5f);
    assert(out[5278 * 2] == 0.25f);                     // last frame that has a successor
    std::printf("ok: sound_lands_at_the_position_it_was_pushed\n");
}

void sixteen_bit_mono_becomes_stereo() {
    start();
    tap::Source* s = tap::attach(48000);
    std::vector<int16_t> in(480, 16384); // 0.5
    tap::push_at(s, in.data(), 480, 1, tap::Fmt::S16, 0);
    auto out = read(0, 300);
    assert(std::fabs(out[10 * 2] - 0.5f) < 1e-4f && std::fabs(out[10 * 2 + 1] - 0.5f) < 1e-4f);
    std::printf("ok: sixteen_bit_mono_becomes_stereo\n");
}

void two_streams_are_summed_and_a_sum_over_full_scale_is_clipped() {
    start();
    tap::Source* a = tap::attach(48000);
    tap::Source* b = tap::attach(48000);
    assert(a && b && a != b);
    push_f32(a, std::vector<float>(480 * 2, 0.2f), 0);
    push_f32(b, std::vector<float>(480 * 2, 0.3f), 0);
    assert(std::fabs(read(0, 200)[50 * 2] - 0.5f) < 1e-6f);
    start();
    a = tap::attach(48000);
    b = tap::attach(48000);
    push_f32(a, std::vector<float>(480 * 2, 0.9f), 0);
    push_f32(b, std::vector<float>(480 * 2, 0.9f), 0);
    assert(read(0, 100)[20 * 2] == 1.0f);
    std::printf("ok: two_streams_are_summed_and_a_sum_over_full_scale_is_clipped\n");
}

void what_a_stream_did_not_supply_is_silence() {
    start();
    tap::Source* s = tap::attach(48000);
    push_f32(s, std::vector<float>(100 * 2, 0.7f), 0);
    auto out = read(0, 480);
    assert(out[50 * 2] == 0.7f);
    for (int i = 100; i < 480; ++i) assert(out[i * 2] == 0.0f && out[i * 2 + 1] == 0.0f);
    // Asking again for the same positions gives the same answer, and later ones silence.
    assert(read(480, 480)[0] == 0.0f);
    std::printf("ok: what_a_stream_did_not_supply_is_silence\n");
}

void a_half_rate_stream_is_stretched_to_the_output_rate() {
    start();
    tap::Source* s = tap::attach(24000);
    // One second at 24 kHz of a 1 kHz sine, pushed in 10 ms pieces on time.
    std::vector<float> got;
    for (int piece = 0; piece < 100; ++piece) {
        std::vector<float> in(240 * 2);
        for (int i = 0; i < 240; ++i) {
            float v = 0.5f * std::sin(2 * kPi * 1000.0f * (piece * 240 + i) / 24000.0f);
            in[i * 2] = in[i * 2 + 1] = v;
        }
        push_f32(s, in, piece * 10);
    }
    got = read(0, 48000);
    int crossings = 0;
    for (size_t i = 1; i < got.size() / 2; ++i)
        if ((got[(i - 1) * 2] < 0) != (got[i * 2] < 0)) ++crossings;
    assert(crossings > 1900 && crossings < 2100);
    std::printf("ok: a_half_rate_stream_is_stretched_to_the_output_rate (crossings=%d)\n", crossings);
}

void a_backend_that_delivers_in_bursts_comes_out_without_holes() {
    // The measurement that rewrote the reader: PipeWire handed over 2048 frames
    // every 42.67 ms while the pump asked for 10 ms at a time. Here a 440 Hz
    // sine arrives that way, with up to 4 ms of lateness on every burst, and the
    // pump runs on a 10 ms tick reading 80 ms behind the clock, as it does.
    start();
    tap::Source* s = tap::attach(48000);
    const double burst_ms = 2048.0 * 1000.0 / 48000.0;
    int next_burst = 0;
    uint64_t written = 0;
    std::vector<float> out;
    for (int t_ms = 0; t_ms < 2000; t_ms += 10) {
        for (;;) {
            const int jitter = (next_burst * 7) % 5;
            const int64_t due = static_cast<int64_t>(next_burst * burst_ms) + jitter;
            if (due > t_ms) break;
            std::vector<float> in(2048 * 2);
            for (int i = 0; i < 2048; ++i)
                in[i * 2] = in[i * 2 + 1] =
                    0.5f * std::sin(2 * kPi * 440.0f * (next_burst * 2048 + i) / 48000.0f);
            push_f32(s, in, due);
            ++next_burst;
        }
        const int64_t want = (static_cast<int64_t>(t_ms) - 80) * 48;
        if (want > static_cast<int64_t>(written)) {
            auto got = read(written, static_cast<uint32_t>(want - written));
            out.insert(out.end(), got.begin(), got.end());
            written = static_cast<uint64_t>(want);
        }
    }
    // The first frame is at position 0 and everything after is sound: no holes.
    int zeros = 0;
    float biggest_step = 0;
    for (size_t i = 100; i + 1 < out.size() / 2; ++i) {
        if (out[i * 2] == 0.0f && out[i * 2 + 1] == 0.0f) ++zeros;
        biggest_step = std::fmax(biggest_step, std::fabs(out[(i + 1) * 2] - out[i * 2]));
    }
    // A 440 Hz sine at 0.5 never steps by more than 2 pi f a / fs = 0.0288.
    assert(out.size() / 2 > 80000);
    assert(biggest_step < 0.031f);
    assert(zeros < 20); // the sine's own zero crossings, a few per cycle at most
    std::printf("ok: a_backend_that_delivers_in_bursts_comes_out_without_holes (max step %.4f, zeros %d)\n",
                biggest_step, zeros);
}

void a_pause_is_filled_so_what_follows_lands_on_time() {
    // Two pauses: a short one, filled with silence in the ring, and a long one,
    // where the timeline is started again. Either way the next sound plays at
    // the time it was pushed, within the jitter allowance.
    start();
    tap::Source* s = tap::attach(48000);
    push_f32(s, std::vector<float>(480 * 2, 0.5f), 0);      // 0..10 ms
    push_f32(s, std::vector<float>(480 * 2, 0.8f), 100);    // resumes at 100 ms
    auto out = read(0, 12000);
    assert(out[300 * 2] == 0.5f);
    assert(out[2000 * 2] == 0.0f);                           // the pause
    assert(out[4700 * 2] == 0.0f);
    assert(out[4810 * 2] == 0.8f);                           // 100 ms is position 4800
    // The long one. By the time the stream returns, the pump has read what came
    // before, so starting afresh loses nothing.
    push_f32(s, std::vector<float>(480 * 2, 0.9f), 1000);
    out = read(12000, 40000);
    assert(out[1000 * 2] == 0.0f);                           // position 13000: silence
    assert(out[(48010 - 12000) * 2] == 0.9f);                // 1 s is position 48000
    std::printf("ok: a_pause_is_filled_so_what_follows_lands_on_time\n");
}

void a_stream_that_runs_ahead_is_trimmed_back() {
    start();
    tap::Source* s = tap::attach(48000);
    // Pushes arrive ten times faster than their own length: the timeline falls
    // behind the clock by more than 150 ms and the surplus must be dropped.
    for (int i = 0; i < 30; ++i) push_f32(s, std::vector<float>(4800 * 2, 0.3f), i * 10);
    uint64_t kept = s->head.load();
    assert(kept < 30 * 4800);                                // something was dropped
    assert(kept > 3 * 4800);                                 // and not everything
    std::printf("ok: a_stream_that_runs_ahead_is_trimmed_back (kept %llu of %d frames)\n",
                (unsigned long long)kept, 30 * 4800);
}

void a_full_ring_drops_and_counts_instead_of_blocking() {
    start();
    tap::Source* s = tap::attach(48000);
    std::vector<float> in(tap::kRingFrames * 2, 0.1f);
    push_f32(s, in, 0);
    assert(s->dropped.load() == 0);
    push_f32(s, std::vector<float>(1000 * 2, 0.1f), 683);
    assert(s->dropped.load() == 1000);
    std::printf("ok: a_full_ring_drops_and_counts_instead_of_blocking\n");
}

void six_channel_output_is_folded_to_stereo() {
    start();
    tap::Source* s = tap::attach(48000);
    std::vector<float> in(100 * 6, 0.0f);
    for (int i = 0; i < 100; ++i) in[i * 6 + 2] = 0.5f; // centre only
    tap::push_at(s, in.data(), 100, 6, tap::Fmt::F32, 0);
    auto out = read(0, 50);
    assert(std::fabs(out[10 * 2] - 0.35355f) < 1e-3f && std::fabs(out[10 * 2 + 1] - 0.35355f) < 1e-3f);
    std::printf("ok: six_channel_output_is_folded_to_stereo\n");
}

void a_released_slot_is_reused_and_starts_empty() {
    start();
    tap::Source* s = tap::attach(48000);
    push_f32(s, std::vector<float>(480 * 2, 0.8f), 0);
    tap::detach(s);
    tap::Source* t = tap::attach(48000);
    assert(t == s);
    assert(peak(read(0, 100)) == 0.0f);
    std::printf("ok: a_released_slot_is_reused_and_starts_empty\n");
}

void starting_again_forgets_the_earlier_recording() {
    start();
    tap::Source* s = tap::attach(48000);
    push_f32(s, std::vector<float>(4800 * 2, 0.6f), 0);
    assert(read(0, 100)[10 * 2] == 0.6f);
    tap::enable(true);                 // a second recording begins
    tap::state().epoch_ns.store(0);
    assert(peak(read(0, 100)) == 0.0f); // nothing until the stream pushes again
    push_f32(s, std::vector<float>(4800 * 2, 0.4f), 0);
    assert(read(0, 100)[10 * 2] == 0.4f);
    std::printf("ok: starting_again_forgets_the_earlier_recording\n");
}

} // namespace

int main() {
    nothing_is_kept_while_not_recording();
    sound_lands_at_the_position_it_was_pushed();
    sixteen_bit_mono_becomes_stereo();
    two_streams_are_summed_and_a_sum_over_full_scale_is_clipped();
    what_a_stream_did_not_supply_is_silence();
    a_half_rate_stream_is_stretched_to_the_output_rate();
    a_backend_that_delivers_in_bursts_comes_out_without_holes();
    a_pause_is_filled_so_what_follows_lands_on_time();
    a_stream_that_runs_ahead_is_trimmed_back();
    a_full_ring_drops_and_counts_instead_of_blocking();
    six_channel_output_is_folded_to_stereo();
    a_released_slot_is_reused_and_starts_empty();
    starting_again_forgets_the_earlier_recording();
    std::printf("record_tap: all checks passed\n");
    return 0;
}
