// Checks that the buffer-queue fill path in `pipewire_backend.cpp` pads an
// underrun with silence rather than replaying stale buffer contents — see
// the comment on `testing::fill_pcm` in `pipewire_backend.h` for why that
// distinction matters: a periodic replay of old samples is exactly what
// turns a silent gap into an audible tone, and a real one reached a
// developer's speakers once already because a verification harness for this
// backend used a live, audible stream when a check like this one would have
// done the job with no PipeWire session, no hardware and no way to make a
// sound.
//
// Deliberately does not link against a live PipeWire connection: it only
// exercises `testing::fill_pcm()`, the pure copy-and-pad logic
// `Impl::process()` calls every cycle, so it runs anywhere this tree builds
// with `pipewire-devel` present, with no daemon, no session, and nothing
// audible possible.

#include "pipewire_backend.h"

#include <cassert>
#include <chrono>
#include <cstdlib>
#include <cstdio>
#include <cstring>

using cordial::audio::testing::choose_output_target;
using cordial::audio::testing::fill_pcm;
using cordial::audio::testing::PendingBuffer;

namespace {

void a_full_queue_fills_the_buffer_exactly() {
    uint8_t source[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, nullptr});

    uint8_t dst[8];
    std::memset(dst, 0xAA, sizeof(dst));
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    assert(std::memcmp(dst, source, sizeof(source)) == 0);
    assert(drained.size() == 1);
    assert(pending.empty());
    std::printf("ok: a_full_queue_fills_the_buffer_exactly\n");
}

void an_empty_queue_produces_silence_not_stale_bytes() {
    std::deque<PendingBuffer> pending;
    uint8_t dst[8];
    // A recognisable non-zero pattern first, matching what a reused
    // pw_stream buffer would actually hold left over from a previous cycle
    // — the bug this guards against is exactly this pattern surviving into
    // the output.
    std::memset(dst, 0xAA, sizeof(dst));

    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == sizeof(dst));
    for (uint8_t b : dst) {
        assert(b == 0);
    }
    assert(drained.empty());
    std::printf("ok: an_empty_queue_produces_silence_not_stale_bytes\n");
}

void a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence() {
    uint8_t source[3] = {9, 9, 9};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, nullptr});

    uint8_t dst[8];
    std::memset(dst, 0xAA, sizeof(dst));
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 5);
    assert(std::memcmp(dst, source, sizeof(source)) == 0);
    for (size_t i = 3; i < 8; ++i) {
        assert(dst[i] == 0);
    }
    assert(drained.size() == 1);
    std::printf("ok: a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence\n");
}

void a_partially_consumed_buffer_is_not_dropped_early() {
    uint8_t source[4] = {1, 2, 3, 4};
    std::deque<PendingBuffer> pending;
    pending.push_back({source, sizeof(source), 0, reinterpret_cast<void*>(0x1234)});

    uint8_t dst[2];
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    assert(dst[0] == 1 && dst[1] == 2);
    // Not fully consumed yet: still queued, no drain callback due for it.
    assert(drained.empty());
    assert(pending.size() == 1);
    assert(pending.front().offset == 2);
    std::printf("ok: a_partially_consumed_buffer_is_not_dropped_early\n");
}

void multiple_buffers_are_drained_in_order_within_one_fill() {
    uint8_t a[2] = {1, 2};
    uint8_t b[2] = {3, 4};
    std::deque<PendingBuffer> pending;
    pending.push_back({a, sizeof(a), 0, reinterpret_cast<void*>(1)});
    pending.push_back({b, sizeof(b), 0, reinterpret_cast<void*>(2)});

    uint8_t dst[4];
    std::vector<void*> drained;
    uint32_t padded = fill_pcm(pending, dst, sizeof(dst), drained);

    assert(padded == 0);
    uint8_t expect[4] = {1, 2, 3, 4};
    assert(std::memcmp(dst, expect, sizeof(expect)) == 0);
    assert(drained.size() == 2);
    assert(drained[0] == reinterpret_cast<void*>(1));
    assert(drained[1] == reinterpret_cast<void*>(2));
    assert(pending.empty());
    std::printf("ok: multiple_buffers_are_drained_in_order_within_one_fill\n");
}

// The privacy rule, checked the only way it can be checked without a session:
// by construction. A `CaptureStream` that has not been opened must hold no
// PipeWire resource, and the process-wide count of open capture streams must
// still be zero after one has been built and destroyed. This does not prove
// the live case — `pw-top` and the desktop's microphone indicator do that, and
// the report accompanying this change carries that evidence — but it does pin
// the invariant that the live case rests on, and it will fail loudly if
// someone later moves the `open()` call into the constructor.
void a_capture_stream_holds_nothing_until_it_is_opened() {
    assert(cordial::audio::active_capture_streams() == 0);
    {
        cordial::audio::CaptureStream capture;
        assert(!capture.is_open());
        assert(cordial::audio::active_capture_streams() == 0);

        // A read from a stream that was never opened returns no samples
        // rather than blocking for some that will never arrive, and must not
        // open anything in order to answer.
        uint8_t buf[64];
        std::memset(buf, 0xAA, sizeof(buf));
        assert(capture.read(buf, sizeof(buf)) == 0);
        assert(cordial::audio::active_capture_streams() == 0);
    }
    assert(cordial::audio::active_capture_streams() == 0);
    std::printf("ok: a_capture_stream_holds_nothing_until_it_is_opened\n");
}

// ------------------------------------------------------- output device choice
//
// The three answers a device picker has to get right, none of which needs a
// session: the chosen sink is present, the chosen sink has gone, and nobody
// chose anything. The third is the default and the one most likely to be
// broken by accident, because "fall back to the default" and "follow the
// default" produce the same empty target and only one of them should warn.

void a_chosen_sink_that_is_present_is_the_target() {
    const std::vector<std::string> sinks = {
        "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__Speaker__sink",
        "bluez_output.AC_12_2F_9E_00_11.1",
    };
    bool fell_back = true; // deliberately wrong to start, so a no-op would fail
    assert(choose_output_target("bluez_output.AC_12_2F_9E_00_11.1", sinks, &fell_back) ==
           "bluez_output.AC_12_2F_9E_00_11.1");
    assert(!fell_back);
    std::printf("ok: a_chosen_sink_that_is_present_is_the_target\n");
}

void a_chosen_sink_that_has_gone_falls_back_to_the_default_and_says_so() {
    // The headset was unplugged since the user chose it. Setting
    // PW_KEY_TARGET_OBJECT to it anyway gives PipeWire nothing to link to and
    // the game plays into nowhere, which is the outcome this whole path exists
    // to prevent.
    const std::vector<std::string> sinks = {"alsa_output.pci-0000_00_1f.3.analog-stereo"};
    bool fell_back = false;
    assert(choose_output_target("bluez_output.AC_12_2F_9E_00_11.1", sinks, &fell_back).empty());
    assert(fell_back);
    std::printf("ok: a_chosen_sink_that_has_gone_falls_back_to_the_default_and_says_so\n");
}

void no_choice_follows_the_default_and_is_not_a_fallback() {
    const std::vector<std::string> sinks = {"alsa_output.pci-0000_00_1f.3.analog-stereo"};
    bool fell_back = true;
    assert(choose_output_target("", sinks, &fell_back).empty());
    // Not a fallback: nothing was lost. If this reported one, every launch by
    // every user who never opened the setting would carry a warning about a
    // missing device, and the warning that means something would be lost in
    // it.
    assert(!fell_back);
    std::printf("ok: no_choice_follows_the_default_and_is_not_a_fallback\n");
}

void an_empty_session_still_falls_back_rather_than_targeting_nothing() {
    // No sinks at all — a daemon that has just started, or one with every
    // device suspended out of the graph. The answer is the same as a missing
    // device and must not be "target it anyway and hope".
    bool fell_back = false;
    assert(choose_output_target("alsa_output.anything", {}, &fell_back).empty());
    assert(fell_back);
    std::printf("ok: an_empty_session_still_falls_back_rather_than_targeting_nothing\n");
}

void matching_is_exact_rather_than_by_prefix_or_description() {
    // Two sinks on one card differ only by a suffix, which is the ordinary
    // case on the machine this was written on: HDMI1, HDMI2, HDMI3 and
    // Speaker all share a long prefix. A prefix match would send audio to
    // whichever happened to be listed first.
    const std::vector<std::string> sinks = {
        "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI1__sink",
        "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI2__sink",
    };
    bool fell_back = false;
    assert(choose_output_target(
               "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI2__sink",
               sinks, &fell_back) ==
           "alsa_output.pci-0000_00_1f.3-platform-skl_hda_dsp_generic.HiFi__HDMI2__sink");
    assert(!fell_back);

    // And the description a user reads is not a routing target.
    fell_back = false;
    assert(choose_output_target("Built-in Audio Digital Stereo (HDMI 2)", sinks, &fell_back)
               .empty());
    assert(fell_back);
    std::printf("ok: matching_is_exact_rather_than_by_prefix_or_description\n");
}

void a_null_fell_back_pointer_is_allowed() {
    // `resolve_output_target` always passes one, but a caller that only wants
    // the target should not have to invent somewhere to put the flag.
    assert(choose_output_target("x", {"x"}, nullptr) == "x");
    assert(choose_output_target("x", {"y"}, nullptr).empty());
    std::printf("ok: a_null_fell_back_pointer_is_allowed\n");
}

/// The shell's C ABI, checked for the two things that can be checked here.
///
/// **Deliberately does not enumerate.** Every other check in this binary runs
/// with no daemon, no session and nothing audible possible, and it runs as a
/// build step — so a check that connected to whatever PipeWire the machine
/// doing the build happens to have would make the build depend on the
/// developer's audio session, and would be the first thing here whose result
/// varied by machine. What it can pin is the contract at the boundary: a null
/// out-pointer is answered rather than dereferenced, and freeing nothing is
/// not a crash. The live behaviour is `enumerate_devices`', and the invariant
/// that matters about it — that listing devices opens no capture stream — is
/// what `a_capture_stream_holds_nothing_until_it_is_opened` above pins.
void the_shells_device_list_survives_being_asked_for_nothing() {
    assert(cordial_audio_sinks(nullptr) == 0);
    cordial_audio_sinks_free(nullptr, 0);
    assert(cordial::audio::active_capture_streams() == 0);
    std::printf("ok: the_shells_device_list_survives_being_asked_for_nothing\n");
}

/// The seam ADR-023 asked for: the caller no longer names a backend, and the
/// factory always hands back something whose `open` can be called.
///
/// Worth a check rather than an assumption because "never null" is the property
/// every call site in `aaudio.cpp` now relies on without testing for it — the
/// failure it forecloses is a null dereference on a machine with no PipeWire,
/// which is the machine least likely to be the one running this test.
void the_factory_always_returns_a_stream() {
    auto stream = cordial::audio::make_output_stream();
    assert(stream != nullptr);
    assert(!stream->is_open());
    assert(!stream->is_running());
    std::printf("ok: the_factory_always_returns_a_stream\n");
}

/// `CORDIAL_AUDIO_HOST` names a backend, falls back rather than failing, and is
/// read exactly once.
///
/// The once matters and is not tidiness: `host_backend_name` caches in a
/// function-local static, so a second reader with a different value would get
/// the first one's answer and no warning. Asserting that the cached answer
/// survives a later `setenv` is what makes that contract visible instead of
/// being a thing somebody discovers by changing the variable at runtime.
void an_unknown_host_backend_falls_back_to_pipewire() {
    // Whatever the environment says, the only backend this build has is
    // PipeWire, so every answer is the same one. ADR-023 schedules PulseAudio
    // and ALSA behind this name.
    const char* first = cordial::audio::host_backend_name();
    assert(std::strcmp(first, "pipewire") == 0);
    ::setenv("CORDIAL_AUDIO_HOST", "something-that-does-not-exist", 1);
    assert(std::strcmp(cordial::audio::host_backend_name(), first) == 0);
    ::unsetenv("CORDIAL_AUDIO_HOST");
    std::printf("ok: an_unknown_host_backend_falls_back_to_pipewire\n");
}

/// A PipeWire that is still starting when sound is first wanted must not cost
/// the whole run its audio. The first attempt fails, the daemon comes up, and a
/// later caller has to get the connection.
void a_connection_that_failed_is_retried_once_the_wait_has_passed() {
    using cordial::audio::testing::RetryingSlot;
    using Clock = std::chrono::steady_clock;
    int session = 0;
    bool daemon_up = false;
    int connects = 0;
    Clock::time_point t{};
    auto now = [&] { return t; };
    auto connect = [&](bool) -> int* {
        ++connects;
        return daemon_up ? &session : nullptr;
    };

    RetryingSlot<int> slot(std::chrono::seconds(5));
    assert(slot.get(connect, now) == nullptr);
    assert(connects == 1);

    // Inside the wait nobody is asked again, even though the daemon is now up:
    // a wedged one costs three seconds a try.
    daemon_up = true;
    t += std::chrono::seconds(2);
    assert(slot.get(connect, now) == nullptr);
    assert(connects == 1);

    t += std::chrono::seconds(4);
    assert(slot.get(connect, now) == &session);
    assert(connects == 2);

    // A success is kept: no further connects, however long it has been.
    t += std::chrono::hours(1);
    assert(slot.get(connect, now) == &session);
    assert(connects == 2);
    std::printf("ok: a_connection_that_failed_is_retried_once_the_wait_has_passed\n");
}

/// The connector is told which attempt it is, so the repeated failures of a
/// daemon that never comes up are logged once rather than on every retry.
void only_the_first_attempt_is_marked_first() {
    using cordial::audio::testing::RetryingSlot;
    using Clock = std::chrono::steady_clock;
    Clock::time_point t{};
    auto now = [&] { return t; };
    std::vector<bool> seen;
    auto connect = [&](bool first) -> int* {
        seen.push_back(first);
        return nullptr;
    };
    RetryingSlot<int> slot(std::chrono::seconds(5));
    for (int i = 0; i < 3; ++i) {
        slot.get(connect, now);
        t += std::chrono::seconds(6);
    }
    assert((seen == std::vector<bool>{true, false, false}));
    assert(slot.attempts() == 3);
    std::printf("ok: only_the_first_attempt_is_marked_first\n");
}

/// The control: the old behaviour was a retry wait of forever, and it is what a
/// slot configured that way still does. Without the retry above, the previous
/// test fails here.
void a_slot_that_never_retries_keeps_its_first_failure() {
    using cordial::audio::testing::RetryingSlot;
    using Clock = std::chrono::steady_clock;
    Clock::time_point t{};
    auto now = [&] { return t; };
    int session = 0;
    int connects = 0;
    bool daemon_up = false;
    auto connect = [&](bool) -> int* {
        ++connects;
        return daemon_up ? &session : nullptr;
    };
    RetryingSlot<int> slot(Clock::duration::max());
    assert(slot.get(connect, now) == nullptr);
    daemon_up = true;
    t += std::chrono::hours(24);
    assert(slot.get(connect, now) == nullptr);
    assert(connects == 1);
    std::printf("ok: a_slot_that_never_retries_keeps_its_first_failure\n");
}

} // namespace

int main() {
    a_full_queue_fills_the_buffer_exactly();
    an_empty_queue_produces_silence_not_stale_bytes();
    a_short_queue_fills_what_it_has_and_pads_the_rest_with_silence();
    a_partially_consumed_buffer_is_not_dropped_early();
    multiple_buffers_are_drained_in_order_within_one_fill();
    a_capture_stream_holds_nothing_until_it_is_opened();
    a_chosen_sink_that_is_present_is_the_target();
    a_chosen_sink_that_has_gone_falls_back_to_the_default_and_says_so();
    no_choice_follows_the_default_and_is_not_a_fallback();
    an_empty_session_still_falls_back_rather_than_targeting_nothing();
    matching_is_exact_rather_than_by_prefix_or_description();
    a_null_fell_back_pointer_is_allowed();
    the_shells_device_list_survives_being_asked_for_nothing();
    the_factory_always_returns_a_stream();
    an_unknown_host_backend_falls_back_to_pipewire();
    a_connection_that_failed_is_retried_once_the_wait_has_passed();
    only_the_first_attempt_is_marked_first();
    a_slot_that_never_retries_keeps_its_first_failure();
    std::printf("all pipewire_backend checks passed\n");
    return 0;
}
