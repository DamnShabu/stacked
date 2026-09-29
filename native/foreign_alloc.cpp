// libc calls that hand the caller a heap block and expect the caller to free it.
//
// Roblox links its own allocator into the engine (mimalloc, going by
// `DFLog::Mimalloc`; docs/adr/ADR-040) and imports no `malloc` or `free` at
// all, so every `free` the engine makes runs inside that allocator. A pointer
// that glibc's `malloc` produced was never registered with it, and the free
// faults. `s_realpath` in system_paths.cpp is where that was first caught, live
// under lldb, on `realpath(path, NULL)`; this file is the same fix for the other
// two imports that have the shape.
//
// There is no buffer Cordial can hand back in their place: the engine's
// allocator is not exported, so nothing here can obtain a block it will accept.
// The only safe move is the one `s_realpath` makes, never to produce the host
// allocation, and to report the failure each function is documented to report
// when it cannot allocate.
//
// **What was observed and what was not.** With `--host-libc`, both resolved
// straight to glibc (read back from the engine's own GOT under gdb, 2026-09-29).
// Over two signed-out runs of 40 s and 110 s -- startup, the landing page and
// the sign-in form, under gdb breakpoints on the host entry points -- the engine
// made four `getcwd` calls in each, all with a caller buffer of 128 bytes, and
// no `vasprintf` call at all. So neither refusal below was seen to fire. They are
// here on INFERRED grounds: both are imported, both allocate in the form that
// matters, an engine that calls either and then frees the result faults the way
// `realpath` did, and a signed-in game session was not exercised. The first
// eight refusals of each are logged to stderr, so a run that does reach one says
// so instead of leaving it to be inferred again.
//
// What was checked and not guarded: `sscanf`, `fscanf` and `vsscanf` allocate
// for a `%m` conversion. In the 110 s run the engine made 34,049 `fscanf` and
// 1,546 `sscanf` calls and none carried one, `vsscanf` was never called, and
// `libroblox.so` has no `%ms`, `%mc` or `%m[` format string in its read-only
// data. Wrapping the variadic scanf family to
// police a format nobody uses, on a path that ran 34,000 times in a hundred
// seconds, was judged worse than the risk.

#include <atomic>
#include <cerrno>
#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <unistd.h>

namespace {

/// How many refusals of each kind reach the log. Enough to show that a code
/// path is live and what it asked for; not enough to flood stderr if the engine
/// calls one in a loop.
constexpr int kLogLimit = 8;

std::atomic<int> g_vasprintf_logged{0};
std::atomic<int> g_getcwd_logged{0};

bool may_log(std::atomic<int>& counter) {
    return counter.fetch_add(1, std::memory_order_relaxed) < kLogLimit;
}

} // namespace

extern "C" {

/// `vasprintf`, which always allocates its result and so has no form the engine
/// can be given safely.
///
/// glibc documents failure as "-1, and the contents of `*strp` are undefined",
/// with `ENOMEM` for an allocation that was not possible, which is precisely the
/// case. `*strp` is set to null rather than left alone: a caller that ignores
/// the return and frees it anyway then frees null, where an untouched stack slot
/// would be freed as garbage.
///
/// The format is logged and the arguments are not: the arguments are whatever
/// the engine was formatting, and a log line is the wrong place for them.
int cordial_vasprintf(char** strp, const char* fmt, va_list ap) {
    (void)ap;
    if (strp) {
        *strp = nullptr;
    }
    if (may_log(g_vasprintf_logged)) {
        std::fprintf(stderr,
                     "[alloc] vasprintf(fmt=\"%.80s\") refused with ENOMEM: a glibc block "
                     "cannot be freed by the engine's allocator (this path was inferred "
                     "unreachable, and has now been reached)\n",
                     fmt ? fmt : "(null)");
    }
    errno = ENOMEM;
    return -1;
}

/// `getcwd`, whose caller-buffer form is forwarded untouched.
///
/// A null `buf` is the Linux and bionic extension in which the library
/// `malloc`s the result, and that is the form refused. `size` is ignored for it:
/// zero means "as large as needed" and any other value means "this large", and
/// both allocate.
///
/// `ENOMEM` is the errno `getcwd` is documented to set when it could not
/// allocate the buffer for this form, so a caller that inspects it reads the
/// truth about what did not happen. A non-null buffer keeps every glibc result,
/// including `EINVAL` for a zero size and `ERANGE` for one that is too small.
char* cordial_getcwd(char* buf, size_t size) {
    if (!buf) {
        if (may_log(g_getcwd_logged)) {
            std::fprintf(stderr,
                         "[alloc] getcwd(NULL, %zu) refused with ENOMEM: a glibc block cannot "
                         "be freed by the engine's allocator (this path was inferred unreachable, "
                         "and has now been reached)\n",
                         size);
        }
        errno = ENOMEM;
        return nullptr;
    }
    return ::getcwd(buf, size);
}

struct CordialAllocSymbol {
    const char* name;
    void* addr;
};

const CordialAllocSymbol* cordial_alloc_symbols(size_t* count) {
    static const CordialAllocSymbol table[] = {
        {"vasprintf", reinterpret_cast<void*>(&cordial_vasprintf)},
        {"getcwd", reinterpret_cast<void*>(&cordial_getcwd)},
    };
    *count = sizeof(table) / sizeof(table[0]);
    return table;
}

} // extern "C"
