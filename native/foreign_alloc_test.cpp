// Checks for foreign_alloc.cpp. Built with -UNDEBUG and run as a post-build
// step, so a wrong refusal fails the build instead of shipping.
//
// What these establish is the *contract* of the two shims -- the return value,
// the errno and the out-parameter each function is documented to give -- and
// that the refusal leaves no glibc block behind. They do not establish that the
// engine calls either function in the allocating form; that is the gdb
// observation recorded in foreign_alloc.cpp, and it found no such call.

#include <cassert>
#include <cerrno>
#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstring>
#include <initializer_list>
#include <malloc.h>
#include <unistd.h>

extern "C" {
int cordial_vasprintf(char** strp, const char* fmt, va_list ap);
char* cordial_getcwd(char* buf, size_t size);
struct CordialAllocSymbol {
    const char* name;
    void* addr;
};
const CordialAllocSymbol* cordial_alloc_symbols(size_t* count);
}

static int call_vasprintf(char** strp, const char* fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    const int r = cordial_vasprintf(strp, fmt, ap);
    va_end(ap);
    return r;
}

/// Bytes glibc's allocator has handed out and not had returned. Compared before
/// and after a refusal to show that nothing escaped, which a return value alone
/// would not: a shim could return -1 and still have leaked the block it built.
static size_t in_use() {
#if defined(__GLIBC__) && (__GLIBC__ > 2 || (__GLIBC__ == 2 && __GLIBC_MINOR__ >= 33))
    return mallinfo2().uordblks;
#else
    return 0;
#endif
}

int main() {
    // vasprintf: -1, ENOMEM, *strp cleared -- never a glibc block.
    {
        char* s = reinterpret_cast<char*>(0xdead);
        errno = 0;
        const size_t before = in_use();
        const int r = call_vasprintf(&s, "%s=%d", "answer", 42);
        assert(r == -1);
        assert(errno == ENOMEM);
        assert(s == nullptr);
        assert(in_use() == before);
    }
    // A null out-parameter is refused the same way rather than dereferenced.
    {
        errno = 0;
        assert(call_vasprintf(nullptr, "x") == -1);
        assert(errno == ENOMEM);
    }
    // Repeated refusals keep refusing after the log budget is spent.
    for (int i = 0; i < 20; i++) {
        char* s = reinterpret_cast<char*>(0xdead);
        assert(call_vasprintf(&s, "%d", i) == -1);
        assert(s == nullptr);
    }

    // getcwd, NULL buffer: the allocating form, refused with ENOMEM, both
    // spellings of size.
    for (size_t size : {size_t(0), size_t(1), size_t(4096)}) {
        errno = 0;
        const size_t before = in_use();
        assert(cordial_getcwd(nullptr, size) == nullptr);
        assert(errno == ENOMEM);
        assert(in_use() == before);
    }

    // getcwd, caller buffer: glibc's answer, byte for byte.
    {
        char want[4096], got[4096];
        assert(::getcwd(want, sizeof want) == want);
        std::memset(got, 0x7f, sizeof got);
        assert(cordial_getcwd(got, sizeof got) == got);
        assert(std::strcmp(want, got) == 0);
    }
    // ...and glibc's errors, unchanged. A zero size is EINVAL, and a buffer
    // shorter than the path is ERANGE.
    {
        char buf[4096];
        errno = 0;
        assert(cordial_getcwd(buf, 0) == nullptr);
        assert(errno == EINVAL);
        char tiny[1];
        errno = 0;
        assert(cordial_getcwd(tiny, sizeof tiny) == nullptr);
        assert(errno == ERANGE);
    }

    // The table the runtime registers names both, and points at these functions.
    {
        size_t count = 0;
        const CordialAllocSymbol* t = cordial_alloc_symbols(&count);
        assert(count == 2);
        bool saw_vasprintf = false, saw_getcwd = false;
        for (size_t i = 0; i < count; i++) {
            if (!std::strcmp(t[i].name, "vasprintf")) {
                saw_vasprintf = t[i].addr == reinterpret_cast<void*>(&cordial_vasprintf);
            }
            if (!std::strcmp(t[i].name, "getcwd")) {
                saw_getcwd = t[i].addr == reinterpret_cast<void*>(&cordial_getcwd);
            }
        }
        assert(saw_vasprintf && saw_getcwd);
    }

    std::puts("foreign_alloc: ok");
    return 0;
}
