#include "pipewire_backend.h"

#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

struct AAudioStreamBuilder;
struct AAudioStream;

namespace cordial::audio {
namespace {

std::atomic<uint32_t> g_capture_count{0};
std::atomic<bool> g_stall_capture{false};
std::atomic<bool> g_capture_failed{false};

class FakeOutputStream final : public OutputStream {
public:
    bool open(uint32_t, bool, const char*, const char*, FillCallback, void*) override {
        return false;
    }
    void close() override {}
    bool is_open() const override { return false; }
    void set_running(bool) override {}
    bool is_running() const override { return false; }
    uint32_t rate_hz() const override { return 0; }
    uint32_t channels() const override { return 0; }
    uint32_t sample_bits() const override { return 0; }
    bool sample_is_float() const override { return false; }
    uint32_t burst_frames() const override { return 0; }
    uint64_t silence_cycles() const override { return 0; }
};

} // namespace

struct CaptureStream::Impl {
    std::atomic<bool> open{false};
    std::atomic<uint64_t> next_byte{0};
};

CaptureStream::CaptureStream() : impl_(new Impl()) {}
CaptureStream::~CaptureStream() {
    close();
    delete impl_;
}

bool CaptureStream::open(uint32_t rate, uint32_t channels, const std::string&) {
    if (rate != 48000 || channels != 1) return false;
    if (!impl_->open.exchange(true)) g_capture_count.fetch_add(1);
    impl_->next_byte.store(0);
    g_capture_failed.store(false);
    return true;
}

void CaptureStream::close() {
    if (impl_ && impl_->open.exchange(false)) g_capture_count.fetch_sub(1);
}

bool CaptureStream::is_open() const { return impl_ && impl_->open.load(); }
bool CaptureStream::failed() const { return g_capture_failed.load(); }

uint32_t CaptureStream::read(void* dst, uint32_t size) {
    if (!is_open() || !dst || g_stall_capture.load()) return 0;
    auto* bytes = static_cast<uint8_t*>(dst);
    const uint64_t first = impl_->next_byte.fetch_add(size);
    for (uint32_t i = 0; i < size; ++i) bytes[i] = static_cast<uint8_t>((first + i) & 0xff);
    return size;
}

uint64_t CaptureStream::dropped_bytes() const { return 0; }
uint32_t active_capture_streams() { return g_capture_count.load(); }
bool host_backend_available() { return true; }
const std::string& configured_output_device() {
    static const std::string empty;
    return empty;
}
// No microphone choice in this fake: `aaudio.cpp` asks for one at the moment it
// opens a capture stream, and the answer that keeps this test about the ABI is
// "follow the default".
const std::string& configured_input_device() {
    static const std::string empty;
    return empty;
}
std::string resolve_input_target(const std::string&) { return {}; }
const char* host_backend_name() { return "fake"; }
const char* effective_backend_name() { return "fake"; }
std::unique_ptr<OutputStream> make_output_stream() { return std::make_unique<FakeOutputStream>(); }

} // namespace cordial::audio

extern "C" {

struct CordialAAudioSymbol {
    const char* name;
    void* address;
};

const CordialAAudioSymbol* cordial_aaudio_symbols(size_t* count);

} // extern "C"

namespace {

template <typename Function>
Function symbol(const char* name) {
    size_t count = 0;
    const CordialAAudioSymbol* symbols = cordial_aaudio_symbols(&count);
    for (size_t i = 0; i < count; ++i) {
        if (std::strcmp(symbols[i].name, name) == 0) {
            return reinterpret_cast<Function>(symbols[i].address);
        }
    }
    assert(false && "AAudio symbol missing from virtual library");
    return nullptr;
}

using CreateBuilder = int32_t (*)(AAudioStreamBuilder**);
using DeleteBuilder = int32_t (*)(AAudioStreamBuilder*);
using SetDirection = void (*)(AAudioStreamBuilder*, int32_t);
using SetDataCallback = void (*)(AAudioStreamBuilder*,
                                 int32_t (*)(AAudioStream*, void*, void*, int32_t), void*);
using SetErrorCallback = void (*)(AAudioStreamBuilder*,
                                  void (*)(AAudioStream*, void*, int32_t), void*);
using OpenStream = int32_t (*)(AAudioStreamBuilder*, AAudioStream**);
using Request = int32_t (*)(AAudioStream*);
using Close = int32_t (*)(AAudioStream*);
using GetInt = int32_t (*)(AAudioStream*);
using Read = int32_t (*)(AAudioStream*, void*, int32_t, int64_t);

struct Api {
    CreateBuilder create = symbol<CreateBuilder>("AAudio_createStreamBuilder");
    DeleteBuilder delete_builder = symbol<DeleteBuilder>("AAudioStreamBuilder_delete");
    SetDirection set_direction = symbol<SetDirection>("AAudioStreamBuilder_setDirection");
    SetDataCallback set_callback = symbol<SetDataCallback>("AAudioStreamBuilder_setDataCallback");
    SetErrorCallback set_error_callback =
        symbol<SetErrorCallback>("AAudioStreamBuilder_setErrorCallback");
    OpenStream open = symbol<OpenStream>("AAudioStreamBuilder_openStream");
    Request start = symbol<Request>("AAudioStream_requestStart");
    Request pause = symbol<Request>("AAudioStream_requestPause");
    Request stop = symbol<Request>("AAudioStream_requestStop");
    Close close = symbol<Close>("AAudioStream_close");
    GetInt state = symbol<GetInt>("AAudioStream_getState");
    GetInt rate = symbol<GetInt>("AAudioStream_getSampleRate");
    GetInt channels = symbol<GetInt>("AAudioStream_getChannelCount");
    GetInt format = symbol<GetInt>("AAudioStream_getFormat");
    Read read = symbol<Read>("AAudioStream_read");
};

struct CallbackState {
    std::mutex mutex;
    std::condition_variable changed;
    uint32_t calls = 0;
    int32_t frames = 0;
    bool bytes_match = false;
    int32_t result = 1;
    Request request_from_callback = nullptr;
    int32_t request_result = INT32_MIN;
    uint32_t continue_calls_before_stop = 0;
    bool first_call_stops_then_continues = false;
    bool wait_after_request = false;
    bool request_completed = false;
    bool release_callback = false;
    uint32_t errors = 0;
    int32_t last_error = 0;
};

int32_t receive(AAudioStream* stream, void* opaque, void* audio, int32_t frames) {
    auto* state = static_cast<CallbackState*>(opaque);
    const auto* bytes = static_cast<const uint8_t*>(audio);
    const int32_t request_result =
        state->request_from_callback ? state->request_from_callback(stream) : INT32_MIN;
    std::unique_lock<std::mutex> lock(state->mutex);
    const uint32_t call = ++state->calls;
    bool match = frames == 480;
    const size_t first = static_cast<size_t>(call - 1) * 480 * 2;
    for (int32_t i = 0; i < frames * 2; ++i) {
        if (bytes[i] != static_cast<uint8_t>((first + static_cast<size_t>(i)) & 0xff)) {
            match = false;
        }
    }
    state->frames = frames;
    state->bytes_match = call == 1 ? match : state->bytes_match && match;
    state->request_result = request_result;
    state->request_completed = state->request_from_callback != nullptr;
    state->changed.notify_all();
    if (state->wait_after_request) {
        state->changed.wait(lock, [&] { return state->release_callback; });
    }
    if (state->first_call_stops_then_continues) return call == 1 ? 1 : 0;
    return state->continue_calls_before_stop != 0 && call <= state->continue_calls_before_stop
               ? 0
               : state->result;
}

void release_callback(CallbackState& state) {
    {
        std::lock_guard<std::mutex> lock(state.mutex);
        state.release_callback = true;
    }
    state.changed.notify_all();
}

bool wait_for_request(CallbackState& state) {
    std::unique_lock<std::mutex> lock(state.mutex);
    return state.changed.wait_for(lock, std::chrono::seconds(2), [&] {
        return state.request_completed;
    });
}

void receive_error(AAudioStream*, void* opaque, int32_t error) {
    auto* state = static_cast<CallbackState*>(opaque);
    {
        std::lock_guard<std::mutex> lock(state->mutex);
        ++state->errors;
        state->last_error = error;
    }
    state->changed.notify_all();
}

bool wait_for_errors(CallbackState& state, uint32_t count) {
    std::unique_lock<std::mutex> lock(state.mutex);
    return state.changed.wait_for(lock, std::chrono::seconds(2), [&] {
        return state.errors >= count;
    });
}

bool wait_for_calls(CallbackState& state, uint32_t count) {
    std::unique_lock<std::mutex> lock(state.mutex);
    return state.changed.wait_for(lock, std::chrono::seconds(2), [&] {
        return state.calls >= count;
    });
}

AAudioStream* open_input(Api& api, CallbackState* callback) {
    AAudioStreamBuilder* builder = nullptr;
    assert(api.create(&builder) == 0);
    api.set_direction(builder, 1);
    if (callback) {
        api.set_callback(builder, &receive, callback);
        api.set_error_callback(builder, &receive_error, callback);
    }
    AAudioStream* stream = nullptr;
    assert(api.open(builder, &stream) == 0);
    assert(api.delete_builder(builder) == 0);
    assert(stream != nullptr);
    assert(api.rate(stream) == 48000);
    assert(api.channels(stream) == 1);
    assert(api.format(stream) == 1);
    assert(api.state(stream) == 2);
    assert(cordial::audio::active_capture_streams() == 0);
    return stream;
}

void callback_input_delivers_s16_frames_and_callback_stop_releases_capture() {
    Api api;
    CallbackState callback;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (cordial::audio::active_capture_streams() != 0 &&
           std::chrono::steady_clock::now() < deadline) {
    }
    assert(callback.calls == 1);
    assert(callback.frames == 480);
    assert(callback.bytes_match);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: callback_input_delivers_s16_frames_and_callback_stop_releases_capture\n");
}

void stop_from_inside_callback_does_not_self_join_or_leak_capture() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    callback.request_from_callback = api.stop;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (cordial::audio::active_capture_streams() != 0 &&
           std::chrono::steady_clock::now() < deadline) {
    }
    assert(callback.request_result == 0);
    assert(callback.calls == 1);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: stop_from_inside_callback_does_not_self_join_or_leak_capture\n");
}

void callback_stop_closes_capture_before_a_blocked_callback_returns() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    callback.request_from_callback = api.stop;
    callback.wait_after_request = true;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_request(callback));

    assert(callback.request_result == 0);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0 &&
           "callback-thread requestStop returned while the microphone remained open");
    release_callback(callback);
    assert(api.close(stream) == 0);
    std::printf("ok: callback_stop_closes_capture_before_a_blocked_callback_returns\n");
}

void callback_pause_closes_capture_before_a_blocked_callback_returns() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    callback.request_from_callback = api.pause;
    callback.wait_after_request = true;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_request(callback));

    assert(callback.request_result == 0);
    assert(api.state(stream) == 6);
    assert(cordial::audio::active_capture_streams() == 0 &&
           "callback-thread requestPause returned while the microphone remained open");
    release_callback(callback);
    assert(api.close(stream) == 0);
    std::printf("ok: callback_pause_closes_capture_before_a_blocked_callback_returns\n");
}

void callback_continue_delivers_multiple_bursts_before_stop() {
    Api api;
    CallbackState callback;
    callback.continue_calls_before_stop = 3;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 4));

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (cordial::audio::active_capture_streams() != 0 &&
           std::chrono::steady_clock::now() < deadline) {
    }
    assert(callback.calls == 4);
    assert(callback.frames == 480);
    assert(callback.bytes_match);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: callback_continue_delivers_multiple_bursts_before_stop\n");
}

void external_close_waits_for_active_callback_then_releases_capture() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    callback.wait_after_request = true;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));
    assert(cordial::audio::active_capture_streams() == 1);

    std::atomic<int32_t> close_result{INT32_MIN};
    std::thread closer([&] { close_result.store(api.close(stream)); });
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (cordial::audio::active_capture_streams() != 0 &&
           std::chrono::steady_clock::now() < deadline) {
    }
    assert(cordial::audio::active_capture_streams() == 0 &&
           "external close left the microphone open while waiting for the callback");
    assert(close_result.load() == INT32_MIN &&
           "external close deleted a stream while its callback was still active");
    release_callback(callback);
    closer.join();

    assert(close_result.load() == 0);
    assert(callback.calls == 1);
    assert(cordial::audio::active_capture_streams() == 0);
    std::printf("ok: external_close_waits_for_active_callback_then_releases_capture\n");
}

void concurrent_callback_stop_and_restart_cannot_close_the_new_capture() {
    Api api;
    CallbackState callback;
    callback.first_call_stops_then_continues = true;
    callback.wait_after_request = true;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));

    std::atomic<bool> restart_entered{false};
    std::atomic<int32_t> restart_result{INT32_MIN};
    std::thread restarter([&] {
        restart_entered.store(true);
        restart_result.store(api.start(stream));
    });
    const auto entered_deadline =
        std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!restart_entered.load() && std::chrono::steady_clock::now() < entered_deadline) {
    }
    assert(restart_entered.load());
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    assert(restart_result.load() == INT32_MIN &&
           "requestStart returned before the concurrent callback STOP completed");

    release_callback(callback);
    restarter.join();
    assert(restart_result.load() == 0);
    assert(api.state(stream) == 4);
    assert(cordial::audio::active_capture_streams() == 1 &&
           "the old callback exit closed the newly restarted capture");
    assert(api.stop(stream) == 0);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: concurrent_callback_stop_and_restart_cannot_close_the_new_capture\n");
}

void capture_failure_stops_callback_and_reports_disconnected() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));

    cordial::audio::g_stall_capture.store(true);
    cordial::audio::g_capture_failed.store(true);
    assert(wait_for_errors(callback, 1) &&
           "PipeWire capture failure never reached the AAudio error callback");
    cordial::audio::g_stall_capture.store(false);

    assert(callback.last_error == -899);
    assert(api.state(stream) == 13);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: capture_failure_stops_callback_and_reports_disconnected\n");
}

void close_from_inside_callback_is_refused_without_deleting_the_live_stream() {
    Api api;
    CallbackState callback;
    callback.request_from_callback = api.close;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (cordial::audio::active_capture_streams() != 0 &&
           std::chrono::steady_clock::now() < deadline) {
    }
    assert(callback.request_result == -895);
    assert(callback.calls == 1);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: close_from_inside_callback_is_refused_without_deleting_the_live_stream\n");
}

void pause_cancels_capture_and_start_reopens_it() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, 1));
    assert(api.pause(stream) == 0);
    assert(api.state(stream) == 6);
    assert(cordial::audio::active_capture_streams() == 0);

    const uint32_t first_calls = callback.calls;
    assert(api.start(stream) == 0);
    assert(wait_for_calls(callback, first_calls + 1));
    assert(api.stop(stream) == 0);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: pause_cancels_capture_and_start_reopens_it\n");
}

void stop_cancels_an_idle_callback_and_releases_capture_promptly() {
    Api api;
    CallbackState callback;
    callback.result = 0;
    cordial::audio::g_stall_capture.store(true);
    AAudioStream* stream = open_input(api, &callback);
    assert(api.start(stream) == 0);
    assert(cordial::audio::active_capture_streams() == 1);

    const auto before = std::chrono::steady_clock::now();
    assert(api.stop(stream) == 0);
    const auto elapsed = std::chrono::steady_clock::now() - before;
    cordial::audio::g_stall_capture.store(false);

    assert(elapsed < std::chrono::milliseconds(250));
    assert(callback.calls == 0);
    assert(api.state(stream) == 10);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: stop_cancels_an_idle_callback_and_releases_capture_promptly\n");
}

void blocking_reads_remain_supported() {
    Api api;
    AAudioStream* stream = open_input(api, nullptr);
    assert(api.start(stream) == 0);
    uint8_t bytes[64]{};
    assert(api.read(stream, bytes, 32, 0) == 32);
    for (size_t i = 0; i < sizeof(bytes); ++i) assert(bytes[i] == static_cast<uint8_t>(i));
    assert(api.stop(stream) == 0);
    assert(cordial::audio::active_capture_streams() == 0);
    assert(api.close(stream) == 0);
    std::printf("ok: blocking_reads_remain_supported\n");
}

} // namespace

int main() {
    ::setenv("CORDIAL_AUDIO", "aaudio", 1);
    callback_input_delivers_s16_frames_and_callback_stop_releases_capture();
    stop_from_inside_callback_does_not_self_join_or_leak_capture();
    callback_stop_closes_capture_before_a_blocked_callback_returns();
    callback_pause_closes_capture_before_a_blocked_callback_returns();
    callback_continue_delivers_multiple_bursts_before_stop();
    external_close_waits_for_active_callback_then_releases_capture();
    concurrent_callback_stop_and_restart_cannot_close_the_new_capture();
    capture_failure_stops_callback_and_reports_disconnected();
    close_from_inside_callback_is_refused_without_deleting_the_live_stream();
    pause_cancels_capture_and_start_reopens_it();
    stop_cancels_an_idle_callback_and_releases_capture_promptly();
    blocking_reads_remain_supported();
    std::printf("all AAudio input ABI checks passed\n");
    return 0;
}
