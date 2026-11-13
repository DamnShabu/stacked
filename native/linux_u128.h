// `__u128` and `__s128` for the bionic linker port, which never sees the
// kernel's own <linux/types.h>: the submodule's include/linux/types.h stands in
// for it and defines only the 8- to 64-bit types. Newer arm64 kernel headers
// declare <asm/sigcontext.h>'s vector registers as `__u128 vregs[32]`, and
// linker.cpp reaches that through <sys/param.h> -> <signal.h>. The Fedora 44
// aarch64 rpm built on 2026-10-26 and failed from 2026-11-04 with "unknown
// type name '__u128'", after its image's headers moved. Forced into the
// `linker` target by native/CMakeLists.txt.
#pragma once

#ifdef __SIZEOF_INT128__
typedef unsigned __int128 __u128;
typedef __int128 __s128;
#endif
