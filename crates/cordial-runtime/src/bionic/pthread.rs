//! bionic's pthread surface, answered here rather than by a host library or a
//! stub.
//!
//! Two opposite reasons to be in this file. A type whose layout differs between
//! the libcs is *wrapped*: the bionic-sized object becomes a handle and the real
//! glibc object lives on the heap behind it. A type that is laid out the same is
//! *forwarded* straight to the host — those entry points are here only because
//! the alternative was a generated stub, and for `pthread_once` and
//! thread-specific data a stub is fatal.
//!
//! Sizes in bytes, measured rather than read off. One probe translation unit
//! declaring `char sz_x[sizeof(x)];` per type, compiled once per libc and the
//! symbol sizes read back with `nm -S`. The bionic columns are against this
//! tree's `third_party/mcpelauncher-linker/bionic/libc/include` at
//! `-D__ANDROID_API__=33` and the architecture's own `-target *-linux-android`
//! triple; the glibc columns are against that architecture's own real glibc
//! (2.43, both times this was written). The x86_64 pair was the original
//! measurement; the aarch64 pair was added on 2026-09-23, compiled x86_64
//! natively and aarch64 inside a Fedora 44 container under qemu emulation,
//! since this host has no aarch64 glibc-devel of its own — cross-checked
//! against `grep __SIZEOF_PTHREAD_MUTEX_T /usr/include/bits/pthreadtypes-arch.h`
//! inside that container, which agrees with the `nm` reading exactly.
//!
//! | type                  | bionic (x86_64) | glibc (x86_64) | bionic (aarch64) | glibc (aarch64) | treatment |
//! |-----------------------|----------------:|---------------:|------------------:|------------------:|-|
//! | `pthread_mutex_t`     |              40 |             40 |                 40 |         **48** | passthrough on x86_64, **wrapped on aarch64** — see below |
//! | `pthread_rwlock_t`    |              56 |             56 |                 56 |             56 | passthrough |
//! | `pthread_attr_t`      |              56 |             56 |                 56 |         **64** | passthrough on x86_64, **wrapped on aarch64** — see below |
//! | `pthread_once_t`      |               4 |              4 |                  4 |              4 | forwarded |
//! | `pthread_key_t`       |               4 |              4 |                  4 |              4 | forwarded |
//! | `pthread_cond_t`      |              48 |             48 |                 48 |             48 | wrapped, on a size that was wrong — see below |
//! | **`sem_t`**           |          **16** |         **32** |             **16** |         **32** | **wrapped** |
//!
//! `sem_t` is a real mismatch on both architectures: glibc's `sem_init` writes
//! 32 bytes into a 16-byte object, so the 16 bytes after it belong to whatever
//! was adjacent. The wrapper is what stops that.
//!
//! **bionic's own sizes are identical across architectures**, and that is not
//! a coincidence worth reading much into: every one of these types is declared
//! in terms of `__LP64__` in
//! `third_party/mcpelauncher-linker/bionic/libc/include/bits/pthread_types.h`,
//! not the specific processor, and both `x86_64-android` and `aarch64-android`
//! are LP64. What varies by architecture is glibc, and on x86_64 it happened
//! to agree with bionic for `pthread_mutex_t` and `pthread_attr_t` — which is
//! exactly what made the x86_64-only measurement look more reassuring than it
//! was; nothing here had ever checked whether that agreement was structural or
//! a coincidence of one architecture's glibc.
//!
//! **It was a coincidence. `pthread_mutex_t` and `pthread_attr_t` were a real
//! size mismatch on aarch64, and this was found before either was wrapped.**
//! glibc's aarch64 `pthread_mutex_t` is 48 bytes against bionic's 40; its
//! `pthread_attr_t` is 64 against bionic's 56. Roblox's engine allocates
//! these objects at bionic's size, because that is the only size it knows,
//! so handing either straight to glibc's `pthread_mutex_init` /
//! `pthread_mutex_lock` / … or `pthread_attr_init` / … meant the host libc
//! read and wrote past the end of an object the engine allocated smaller —
//! the same class of overrun `sem_t` already needed a wrapper for, just one
//! that happened not to bite on x86_64. `pthread_mutex_t` is the engine's
//! most heavily used pthread primitive, so this was never a corner case; it
//! was whichever mutex got locked first.
//!
//! **Both are wrapped now**, `BionicMutex`/`BionicAttr` further down this
//! file, aarch64 only — on x86_64 neither type is in [`overrides`] at all,
//! and `pthread_mutex_*`/`pthread_attr_*` keep resolving straight from the
//! host exactly as before this paragraph existed. The mutex wrapper has to
//! handle a mutex that was never explicitly initialised: bionic spells
//! `PTHREAD_MUTEX_INITIALIZER`, `_RECURSIVE_MUTEX_INITIALIZER_NP` and
//! `_ERRORCHECK_MUTEX_INITIALIZER_NP` as three different raw words in the
//! object's own memory, and each has to keep behaving as its declared type
//! on the very first lock, having never passed through `pthread_mutex_init`
//! — see `resolve_mutex`'s own comment for how that differs from
//! `pthread_cond_t`'s and `sem_t`'s single-sentinel lazy init below. The attr
//! wrapper also has to follow `attr` through to `pthread_create` (there is
//! no POSIX static initialiser for an attr, so it has no equivalent
//! complication) — `native/thread_trace.cpp`'s `cordial_pthread_create` calls
//! back into this file, on aarch64 only, to resolve `attr` before forwarding
//! it to the host's own `pthread_create`.
//!
//! **Run, not just typechecked, as of 2026-09-23**: `cargo test --release -p
//! cordial-runtime --lib bionic::pthread::` inside the same emulated aarch64
//! container the size measurement used, natively (the container is aarch64;
//! nothing here is cross-compiled) -- 14 passed, 0 failed, covering a
//! statically-initialised normal/recursive/errorcheck mutex each, a mutex
//! locked and unlocked by eight threads at once, an attr's stack size read
//! back through its own getter, and the same value read back on the far side
//! of a real `pthread_create`/`pthread_getattr_np` round trip through
//! `native/thread_trace.cpp`. **Still not run**: against real ARM64
//! hardware, or against Roblox's own engine -- an emulated qemu-user
//! container proves the ABI translation is right, not that the engine's
//! actual lock usage pattern is.
//!
//! **The `pthread_cond_t` row used to read 32 against 48, and it was wrong.**
//! 32 bytes is `pthread_barrier_t`, which is `int64_t __private[4]`;
//! `pthread_cond_t` is `int32_t __private[12]`, a few declarations further down
//! the same header, and comes to 48 on LP64 — the same as glibc's. The commit
//! that introduced this module recorded the overrun as one of three ABI
//! divergences found, and the measurement above says there was no overrun to
//! find. The wrapper stays for now: it is harmless either way, since it only
//! ever writes the first 12 bytes of the caller's object, and taking it out
//! changes what runs at every `pthread_cond_wait` in the engine — a behaviour
//! change that wants its own measurement rather than a free ride on this one.
//!
//! Initialisation of a wrapper is lazy because bionic's
//! `PTHREAD_COND_INITIALIZER` is all zeroes, so a statically-initialised
//! condition variable can reach `wait` without `init` ever being called.

use std::ffi::{c_int, c_uint, c_void};
#[cfg(target_arch = "aarch64")]
use std::ffi::c_ulong;
use std::sync::atomic::{AtomicU32, Ordering};

/// Marks a wrapper whose backing object exists. Arbitrary, but distinctive in a
/// memory dump and impossible to reach by zero-initialisation.
const READY: u32 = 0xC0D1_A1FF;

const UNINIT: u32 = 0;
const INITIALISING: u32 = 1;

/// An overlay on all 48 bytes of bionic's `pthread_cond_t`, which is
/// `int32_t __private[12]` and so only promises four-byte alignment. Only the
/// first 12 bytes -- `state` and the pointer split across `real_lo` and
/// `real_hi` -- are ours; the reserved words are never touched. The fields are
/// `u32` rather than `u64`/`usize` because reading an eight-byte atomic out of
/// four-byte-aligned storage is undefined behaviour.
#[repr(C)]
struct BionicCond {
    state: AtomicU32,
    real_lo: AtomicU32,
    real_hi: AtomicU32,
    _reserved: [u32; 9],
}

/// bionic's `sem_t`: `unsigned int count; int __reserved[3];`
#[repr(C)]
struct BionicSem {
    state: AtomicU32,
    real_lo: AtomicU32,
    real_hi: AtomicU32,
    _reserved: u32,
}

// glibc's implementations. These resolve to the host's libc at link time; our
// own wrappers are never exported, so there is no recursion.
extern "C" {
    fn pthread_cond_init(cond: *mut c_void, attr: *const c_void) -> c_int;
    fn pthread_cond_destroy(cond: *mut c_void) -> c_int;
    fn pthread_cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int;
    fn pthread_cond_timedwait(cond: *mut c_void, mutex: *mut c_void, ts: *const c_void) -> c_int;
    fn pthread_cond_signal(cond: *mut c_void) -> c_int;
    fn pthread_cond_broadcast(cond: *mut c_void) -> c_int;

    fn sem_init(sem: *mut c_void, pshared: c_int, value: u32) -> c_int;
    fn sem_destroy(sem: *mut c_void) -> c_int;
    fn sem_post(sem: *mut c_void) -> c_int;
    fn sem_wait(sem: *mut c_void) -> c_int;
    fn sem_trywait(sem: *mut c_void) -> c_int;
}

// glibc's mutex/attr/sched implementations, needed only on aarch64 -- see the
// module doc comment's table. Cfg'd out entirely on x86_64, where none of
// this is linked or called: `overrides()` never registers
// `pthread_mutex_*`/`pthread_attr_*`/`pthread_getattr_np` there, so the
// host's own implementations keep resolving directly, unchanged from before
// this fix.
#[cfg(target_arch = "aarch64")]
extern "C" {
    fn pthread_mutex_init(mutex: *mut c_void, attr: *const c_void) -> c_int;
    fn pthread_mutex_destroy(mutex: *mut c_void) -> c_int;
    fn pthread_mutex_lock(mutex: *mut c_void) -> c_int;
    fn pthread_mutex_trylock(mutex: *mut c_void) -> c_int;
    fn pthread_mutex_unlock(mutex: *mut c_void) -> c_int;
    fn pthread_mutex_timedlock(mutex: *mut c_void, abstime: *const c_void) -> c_int;

    fn pthread_mutexattr_init(attr: *mut c_void) -> c_int;
    fn pthread_mutexattr_destroy(attr: *mut c_void) -> c_int;
    fn pthread_mutexattr_settype(attr: *mut c_void, kind: c_int) -> c_int;

    fn pthread_attr_init(attr: *mut c_void) -> c_int;
    fn pthread_attr_destroy(attr: *mut c_void) -> c_int;
    fn pthread_attr_setstacksize(attr: *mut c_void, stacksize: usize) -> c_int;
    fn pthread_attr_getstacksize(attr: *const c_void, stacksize: *mut usize) -> c_int;
    fn pthread_attr_setdetachstate(attr: *mut c_void, state: c_int) -> c_int;
    fn pthread_attr_getdetachstate(attr: *const c_void, state: *mut c_int) -> c_int;
    fn pthread_attr_setguardsize(attr: *mut c_void, guardsize: usize) -> c_int;
    fn pthread_attr_getguardsize(attr: *const c_void, guardsize: *mut usize) -> c_int;
    fn pthread_attr_setschedparam(attr: *mut c_void, param: *const c_void) -> c_int;
    fn pthread_attr_getschedparam(attr: *const c_void, param: *mut c_void) -> c_int;
    fn pthread_attr_setstack(attr: *mut c_void, stackaddr: *mut c_void, stacksize: usize) -> c_int;
    fn pthread_attr_getstack(
        attr: *const c_void,
        stackaddr: *mut *mut c_void,
        stacksize: *mut usize,
    ) -> c_int;
    fn pthread_getattr_np(thread: c_ulong, attr: *mut c_void) -> c_int;
}

/// Size of the heap allocation standing in for a glibc object. Generous on
/// purpose: it costs nothing and removes any chance of repeating the very bug
/// this module fixes if a libc grows its type.
const BACKING_SIZE: usize = 128;

fn alloc_backing() -> *mut c_void {
    let boxed: Box<[u8; BACKING_SIZE]> = Box::new([0u8; BACKING_SIZE]);
    Box::into_raw(boxed) as *mut c_void
}

/// SAFETY: `ptr` must have come from `alloc_backing` and not been freed.
unsafe fn free_backing(ptr: *mut c_void) {
    // SAFETY: `ptr` came from `alloc_backing` and has not been freed, per
    // this function's own contract, stated above.
    drop(unsafe { Box::from_raw(ptr as *mut [u8; BACKING_SIZE]) });
}

/// Resolve the real object behind a wrapper, creating it on first use.
///
/// `init` is called exactly once, with the freshly allocated backing store.
///
/// SAFETY: `state`, `real_lo` and `real_hi` must belong to the same live
/// wrapper object.
unsafe fn resolve(
    state: &AtomicU32,
    real_lo: &AtomicU32,
    real_hi: &AtomicU32,
    init: impl FnOnce(*mut c_void),
) -> *mut c_void {
    loop {
        match state.compare_exchange(UNINIT, INITIALISING, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                let backing = alloc_backing();
                init(backing);
                let address = backing as usize as u64;
                real_lo.store(address as u32, Ordering::Relaxed);
                real_hi.store((address >> 32) as u32, Ordering::Relaxed);
                state.store(READY, Ordering::Release);
                return backing;
            }
            Err(READY) => {
                let address = (real_lo.load(Ordering::Relaxed) as u64)
                    | ((real_hi.load(Ordering::Relaxed) as u64) << 32);
                return address as usize as *mut c_void;
            }
            Err(INITIALISING) => {
                // Another thread is between allocation and publication. This
                // window is a handful of instructions.
                std::hint::spin_loop();
            }
            Err(_) => {
                // Not a value we wrote. The object was not zero-initialised and
                // is not one of ours — most likely a real bug in the caller, but
                // treating it as uninitialised would leak and corrupt. Refuse.
                return std::ptr::null_mut();
            }
        }
    }
}

// ---------------------------------------------------------------- condition vars

/// # Safety
///
/// `cond` must point at storage at least as large as bionic's
/// `pthread_cond_t` (48 bytes; see [`BionicCond`]); `attr`, if non-null, must
/// point at a `pthread_condattr_t` the host's `pthread_cond_init` can read.
/// Both hold here: bionic code only ever hands this its own statically- or
/// explicitly-declared `pthread_cond_t` and `pthread_condattr_t`.
pub unsafe extern "C" fn cond_init(cond: *mut c_void, attr: *const c_void) -> c_int {
    if cond.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 48-byte pthread_cond_t.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    // An explicit init on an object we already wrapped replaces it, matching
    // glibc's "undefined behaviour, but do something sane" posture.
    unsafe { destroy_backing(&c.state, &c.real_lo, &c.real_hi, pthread_cond_destroy) };
    c.state.store(UNINIT, Ordering::Release);
    let backing = unsafe {
        resolve(&c.state, &c.real_lo, &c.real_hi, |p| {
            pthread_cond_init(p, attr);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

pub extern "C" fn cond_destroy(cond: *mut c_void) -> c_int {
    if cond.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    unsafe { destroy_backing(&c.state, &c.real_lo, &c.real_hi, pthread_cond_destroy) };
    0
}

/// Tear down and free a wrapper's backing object, if it has one.
///
/// SAFETY: `state`/`real_lo`/`real_hi` must belong to the same live wrapper.
unsafe fn destroy_backing(
    state: &AtomicU32,
    real_lo: &AtomicU32,
    real_hi: &AtomicU32,
    destroy: unsafe extern "C" fn(*mut c_void) -> c_int,
) {
    if state.swap(UNINIT, Ordering::AcqRel) == READY {
        let low = real_lo.swap(0, Ordering::AcqRel) as u64;
        let high = real_hi.swap(0, Ordering::Acquire) as u64;
        let p = (low | (high << 32)) as usize as *mut c_void;
        if !p.is_null() {
            // SAFETY: `p` was the backing object for this wrapper, and the
            // `state.swap` above already claimed it -- no other caller can
            // observe `READY` again for the same wrapper, so this is the one
            // place that destroys and frees it.
            unsafe {
                destroy(p);
                free_backing(p);
            }
        }
    }
}

macro_rules! cond_op {
    ($name:ident, $glibc:ident) => {
        pub extern "C" fn $name(cond: *mut c_void) -> c_int {
            if cond.is_null() {
                return libc_einval();
            }
            // SAFETY: bionic's contract is a pointer to a 48-byte pthread_cond_t.
            let c = unsafe { &mut *(cond as *mut BionicCond) };
            let backing = unsafe {
                resolve(&c.state, &c.real_lo, &c.real_hi, |p| {
                    pthread_cond_init(p, std::ptr::null());
                })
            };
            if backing.is_null() {
                return libc_einval();
            }
            unsafe { $glibc(backing) }
        }
    };
}

cond_op!(cond_signal, pthread_cond_signal);
cond_op!(cond_broadcast, pthread_cond_broadcast);

/// # Safety
///
/// `cond` must be a bionic `pthread_cond_t` this module has already seen
/// (statically- or explicitly-initialised); `mutex` must point at storage the
/// size of bionic's `pthread_mutex_t` (40 bytes), which is layout-identical to
/// glibc's on x86-64 and so passes straight through.
pub unsafe extern "C" fn cond_wait(cond: *mut c_void, mutex: *mut c_void) -> c_int {
    let Some(backing) = cond_backing(cond) else {
        return libc_einval();
    };
    // SAFETY: `mutex` is a bionic pthread_mutex_t, which is layout-identical to
    // glibc's on x86-64 (both 40 bytes) and so passes straight through.
    unsafe { pthread_cond_wait(backing, mutex) }
}

/// # Safety
///
/// As [`cond_wait`], plus `abstime`, if non-null, must point at a
/// `struct timespec`, which is identical between the two libcs.
pub unsafe extern "C" fn cond_timedwait(
    cond: *mut c_void,
    mutex: *mut c_void,
    abstime: *const c_void,
) -> c_int {
    let Some(backing) = cond_backing(cond) else {
        return libc_einval();
    };
    // SAFETY: as above; `struct timespec` is identical between the two libcs.
    unsafe { pthread_cond_timedwait(backing, mutex, abstime) }
}

fn cond_backing(cond: *mut c_void) -> Option<*mut c_void> {
    if cond.is_null() {
        return None;
    }
    // SAFETY: bionic's contract is a pointer to a 48-byte pthread_cond_t.
    let c = unsafe { &mut *(cond as *mut BionicCond) };
    let backing = unsafe {
        resolve(&c.state, &c.real_lo, &c.real_hi, |p| {
            pthread_cond_init(p, std::ptr::null());
        })
    };
    (!backing.is_null()).then_some(backing)
}

// ------------------------------------------------------------------ semaphores

pub extern "C" fn semaphore_init(sem: *mut c_void, pshared: c_int, value: u32) -> c_int {
    if sem.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 16-byte sem_t.
    let s = unsafe { &mut *(sem as *mut BionicSem) };
    unsafe { destroy_backing(&s.state, &s.real_lo, &s.real_hi, sem_destroy) };
    s.state.store(UNINIT, Ordering::Release);
    let backing = unsafe {
        resolve(&s.state, &s.real_lo, &s.real_hi, |p| {
            sem_init(p, pshared, value);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

pub extern "C" fn semaphore_destroy(sem: *mut c_void) -> c_int {
    if sem.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let s = unsafe { &mut *(sem as *mut BionicSem) };
    unsafe { destroy_backing(&s.state, &s.real_lo, &s.real_hi, sem_destroy) };
    0
}

macro_rules! sem_op {
    ($name:ident, $glibc:ident) => {
        pub extern "C" fn $name(sem: *mut c_void) -> c_int {
            if sem.is_null() {
                return libc_einval();
            }
            // SAFETY: bionic's contract is a pointer to a 16-byte sem_t.
            let s = unsafe { &mut *(sem as *mut BionicSem) };
            // Unlike condition variables a semaphore has no static initialiser,
            // so reaching here uninitialised means sem_init was skipped. Create
            // a zero-count semaphore rather than crashing.
            let backing = unsafe {
                resolve(&s.state, &s.real_lo, &s.real_hi, |p| {
                    sem_init(p, 0, 0);
                })
            };
            if backing.is_null() {
                return libc_einval();
            }
            unsafe { $glibc(backing) }
        }
    };
}

sem_op!(semaphore_post, sem_post);
sem_op!(semaphore_wait, sem_wait);
sem_op!(semaphore_trywait, sem_trywait);

// ------------------------------------------------------------------- mutexes
//
// aarch64 only. glibc's `pthread_mutex_t` is 48 bytes there against bionic's
// 40 (see the size table above); on x86_64 the two agree and this section
// does not exist -- `pthread_mutex_*` is not in `overrides()` there, so the
// host's own implementation keeps resolving directly, exactly as it always
// has for this, the engine's hottest lock.

/// An overlay on all 40 bytes of bionic's `pthread_mutex_t`
/// (`third_party/mcpelauncher-linker/bionic/libc/include/bits/pthread_types.h`:
/// `int32_t __private[10]` on LP64). Same shape as `BionicCond`/`BionicSem`
/// above: `state` carries this file's own lazy-init protocol, `real_lo`/
/// `real_hi` split the backing pointer across two 32-bit atomics because the
/// object only promises four-byte alignment, and the rest is never touched.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
struct BionicMutex {
    state: AtomicU32,
    real_lo: AtomicU32,
    real_hi: AtomicU32,
    _reserved: [u32; 7],
}

/// glibc's own mutex-type numbering. It happens to use the same small
/// integers as bionic's `pthread_mutex.cpp` -- 0/1/2 -- but the two are
/// otherwise unrelated enums; the agreement is not load-bearing anywhere else
/// in this file, only convenient for this comment.
#[cfg(target_arch = "aarch64")]
const GLIBC_MUTEX_RECURSIVE: c_int = 1;
#[cfg(target_arch = "aarch64")]
const GLIBC_MUTEX_ERRORCHECK: c_int = 2;

/// Which mutex type a bionic mutex's *untouched* `state` word asks for.
#[cfg(target_arch = "aarch64")]
#[derive(Clone, Copy)]
enum StaticMutexKind {
    Normal,
    Recursive,
    ErrorCheck,
}

/// bionic's three possible "nobody has called `pthread_mutex_init` yet"
/// values for a mutex's `state` word, straight out of
/// `third_party/mcpelauncher-linker/bionic/libc/bionic/pthread_mutex.cpp`'s
/// `MUTEX_TYPE_SHIFT` (14): `PTHREAD_MUTEX_INITIALIZER`,
/// `_RECURSIVE_MUTEX_INITIALIZER_NP` and `_ERRORCHECK_MUTEX_INITIALIZER_NP`
/// spell `(type & 3) << 14` for type 0, 1 and 2 respectively. Nothing else
/// can be sitting in this word before this file's own wrapper has touched
/// it -- bionic's real mutex algorithm never runs against memory this shim
/// intercepts, and there is no fourth static-initialiser macro to account
/// for.
#[cfg(target_arch = "aarch64")]
fn static_mutex_kind(raw: u32) -> Option<StaticMutexKind> {
    match raw {
        0 => Some(StaticMutexKind::Normal),
        0x4000 => Some(StaticMutexKind::Recursive),
        0x8000 => Some(StaticMutexKind::ErrorCheck),
        _ => None,
    }
}

/// Resolve a mutex's backing object, creating it from bionic's own static
/// pattern the first time it is touched with no explicit `pthread_mutex_init`
/// -- see `static_mutex_kind`. Shaped like [`resolve`] but keyed off three
/// possible "not yet created" values instead of one, because a bionic
/// mutex's untouched state legitimately differs by declared type: an engine
/// mutex declared `PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP` must behave as a
/// recursive mutex on its very first lock, having never passed through
/// `pthread_mutex_init`.
///
/// SAFETY: `m` must be a live `BionicMutex`.
#[cfg(target_arch = "aarch64")]
unsafe fn resolve_mutex(m: &BionicMutex) -> *mut c_void {
    loop {
        let raw = m.state.load(Ordering::Acquire);
        if raw == READY {
            let low = m.real_lo.load(Ordering::Relaxed) as u64;
            let high = m.real_hi.load(Ordering::Relaxed) as u64;
            return (low | (high << 32)) as usize as *mut c_void;
        }
        if raw == INITIALISING {
            // Another thread is between allocation and publication.
            std::hint::spin_loop();
            continue;
        }
        let Some(kind) = static_mutex_kind(raw) else {
            // Not one of the three static patterns and not our own
            // INITIALISING/READY markers -- either genuine corruption or a
            // caller that hand-wrote a bit pattern this shim was never told
            // about. Refuse rather than guess a type, the same posture
            // `resolve`'s own catch-all takes for cond/sem.
            return std::ptr::null_mut();
        };
        if m
            .state
            .compare_exchange(raw, INITIALISING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Lost a race with another thread also resolving this mutex for
            // the first time; reobserve rather than assume who won.
            continue;
        }
        let backing = alloc_backing();
        // SAFETY: `backing` is BACKING_SIZE (128) bytes, comfortably larger
        // than glibc's real `pthread_mutex_t` (48 on aarch64) and its
        // `pthread_mutexattr_t` (a `long`-sized union on any LP64 host).
        unsafe {
            match kind {
                StaticMutexKind::Normal => {
                    pthread_mutex_init(backing, std::ptr::null());
                }
                StaticMutexKind::Recursive | StaticMutexKind::ErrorCheck => {
                    let mut attr_storage = [0u8; BACKING_SIZE];
                    let attr = attr_storage.as_mut_ptr() as *mut c_void;
                    pthread_mutexattr_init(attr);
                    let glibc_kind = if matches!(kind, StaticMutexKind::Recursive) {
                        GLIBC_MUTEX_RECURSIVE
                    } else {
                        GLIBC_MUTEX_ERRORCHECK
                    };
                    pthread_mutexattr_settype(attr, glibc_kind);
                    pthread_mutex_init(backing, attr);
                    pthread_mutexattr_destroy(attr);
                }
            }
        }
        let address = backing as usize as u64;
        m.real_lo.store(address as u32, Ordering::Relaxed);
        m.real_hi.store((address >> 32) as u32, Ordering::Relaxed);
        m.state.store(READY, Ordering::Release);
        return backing;
    }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn mutex_init(mutex: *mut c_void, attr: *const c_void) -> c_int {
    if mutex.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 40-byte pthread_mutex_t.
    let m = unsafe { &mut *(mutex as *mut BionicMutex) };
    // An explicit init on an object we already wrapped replaces it, matching
    // `cond_init`'s posture on the same situation.
    unsafe { destroy_backing(&m.state, &m.real_lo, &m.real_hi, pthread_mutex_destroy) };
    m.state.store(UNINIT, Ordering::Release);
    // `attr` is passed straight through here, unlike `resolve_mutex`'s
    // static-pattern path: an explicitly supplied attr was built by the
    // engine calling `pthread_mutexattr_init`/`_settype`, which this file
    // does not intercept, because bionic's `pthread_mutexattr_t` and
    // glibc's are both a single `long`-sized word on LP64 -- whichever libc's
    // functions wrote it, wrote a self-consistent encoding that the *same*
    // libc's `pthread_mutex_init` reads correctly. Since those attr
    // functions are left unintercepted (ordinary passthrough, resolved from
    // the host), `attr` already holds glibc's own encoding by the time it
    // reaches here, not bionic's, and needs no translation.
    let backing = unsafe {
        resolve(&m.state, &m.real_lo, &m.real_hi, |p| {
            pthread_mutex_init(p, attr);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn mutex_destroy(mutex: *mut c_void) -> c_int {
    if mutex.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let m = unsafe { &mut *(mutex as *mut BionicMutex) };
    unsafe { destroy_backing(&m.state, &m.real_lo, &m.real_hi, pthread_mutex_destroy) };
    0
}

#[cfg(target_arch = "aarch64")]
fn mutex_backing(mutex: *mut c_void) -> Option<*mut c_void> {
    if mutex.is_null() {
        return None;
    }
    // SAFETY: bionic's contract is a pointer to a 40-byte pthread_mutex_t.
    let m = unsafe { &mut *(mutex as *mut BionicMutex) };
    let backing = unsafe { resolve_mutex(m) };
    (!backing.is_null()).then_some(backing)
}

#[cfg(target_arch = "aarch64")]
macro_rules! mutex_op {
    ($name:ident, $glibc:ident) => {
        pub extern "C" fn $name(mutex: *mut c_void) -> c_int {
            let Some(backing) = mutex_backing(mutex) else {
                return libc_einval();
            };
            unsafe { $glibc(backing) }
        }
    };
}

#[cfg(target_arch = "aarch64")]
mutex_op!(mutex_lock, pthread_mutex_lock);
#[cfg(target_arch = "aarch64")]
mutex_op!(mutex_trylock, pthread_mutex_trylock);
#[cfg(target_arch = "aarch64")]
mutex_op!(mutex_unlock, pthread_mutex_unlock);

#[cfg(target_arch = "aarch64")]
pub extern "C" fn mutex_timedlock(mutex: *mut c_void, abstime: *const c_void) -> c_int {
    let Some(backing) = mutex_backing(mutex) else {
        return libc_einval();
    };
    // SAFETY: `struct timespec` is identical between the two libcs, the same
    // assumption `cond_timedwait` already makes above.
    unsafe { pthread_mutex_timedlock(backing, abstime) }
}

// -------------------------------------------------------------- thread attrs
//
// aarch64 only -- 56 bytes in bionic against 64 in glibc; see the module doc
// comment. Unlike a mutex there is no POSIX static initialiser for an attr
// object, so this reuses [`resolve`]/[`destroy_backing`] exactly as `cond`
// and `sem` do, with one addition: `pthread_getattr_np` fills in an attr
// describing an *existing* thread rather than one the caller initialised, so
// it gets its own path below rather than going through `attr_backing`.
//
// This also means `pthread_create` needs to translate whatever `attr` it is
// given before handing it to the host's own `pthread_create` --
// `cordial_pthread_attr_real` below is exported for exactly that, and
// `native/thread_trace.cpp`'s `cordial_pthread_create` calls it on aarch64.
// Missing that call site would leave every other fix in this file correct
// and this one path -- thread creation with a non-default attr, which is
// what a set stack size goes through -- still handing glibc a 56-byte object
// it reads as its own 64-byte one.

/// An overlay on all 56 bytes of bionic's `pthread_attr_t`.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
struct BionicAttr {
    state: AtomicU32,
    real_lo: AtomicU32,
    real_hi: AtomicU32,
    _reserved: [u32; 11],
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_init(attr: *mut c_void) -> c_int {
    if attr.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 56-byte pthread_attr_t.
    let a = unsafe { &mut *(attr as *mut BionicAttr) };
    unsafe { destroy_backing(&a.state, &a.real_lo, &a.real_hi, pthread_attr_destroy) };
    a.state.store(UNINIT, Ordering::Release);
    let backing = unsafe {
        resolve(&a.state, &a.real_lo, &a.real_hi, |p| {
            pthread_attr_init(p);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        0
    }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_destroy(attr: *mut c_void) -> c_int {
    if attr.is_null() {
        return libc_einval();
    }
    // SAFETY: as above.
    let a = unsafe { &mut *(attr as *mut BionicAttr) };
    unsafe { destroy_backing(&a.state, &a.real_lo, &a.real_hi, pthread_attr_destroy) };
    0
}

/// Resolve an attr's backing object, auto-initialising on first touch if a
/// setter or getter is reached with no explicit `pthread_attr_init` --
/// undefined behaviour under POSIX regardless of libc, so this is the same
/// permissive fallback `cond`/`sem` already take rather than a third
/// behaviour introduced just for this type.
///
/// SAFETY: `attr` must be a live, 56-byte `BionicAttr`.
#[cfg(target_arch = "aarch64")]
unsafe fn attr_backing(attr: *const c_void) -> Option<*mut c_void> {
    if attr.is_null() {
        return None;
    }
    // SAFETY: caller's obligation, stated above.
    let a = unsafe { &mut *(attr as *mut BionicAttr) };
    let backing = unsafe {
        resolve(&a.state, &a.real_lo, &a.real_hi, |p| {
            pthread_attr_init(p);
        })
    };
    (!backing.is_null()).then_some(backing)
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_setstacksize(attr: *mut c_void, stacksize: usize) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_setstacksize(backing, stacksize) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_getstacksize(attr: *const c_void, out: *mut usize) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_getstacksize(backing, out) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_setdetachstate(attr: *mut c_void, state: c_int) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_setdetachstate(backing, state) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_getdetachstate(attr: *const c_void, out: *mut c_int) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_getdetachstate(backing, out) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_setguardsize(attr: *mut c_void, guardsize: usize) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_setguardsize(backing, guardsize) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_getguardsize(attr: *const c_void, out: *mut usize) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_getguardsize(backing, out) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_setschedparam(attr: *mut c_void, param: *const c_void) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    // SAFETY: `struct sched_param` is `{ int sched_priority; }` in both
    // libcs on Linux -- INFERRED from the POSIX-mandated field and confirmed
    // against this tree's own vendored bionic header, not independently
    // measured against glibc's the way the pthread types above were.
    unsafe { pthread_attr_setschedparam(backing, param) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_getschedparam(attr: *const c_void, param: *mut c_void) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_getschedparam(backing, param) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_setstack(attr: *mut c_void, stackaddr: *mut c_void, stacksize: usize) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_setstack(backing, stackaddr, stacksize) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn attr_getstack(
    attr: *const c_void,
    stackaddr: *mut *mut c_void,
    stacksize: *mut usize,
) -> c_int {
    let Some(backing) = (unsafe { attr_backing(attr) }) else {
        return libc_einval();
    };
    unsafe { pthread_attr_getstack(backing, stackaddr, stacksize) }
}

#[cfg(target_arch = "aarch64")]
pub extern "C" fn getattr_np(thread: c_ulong, attr: *mut c_void) -> c_int {
    if attr.is_null() {
        return libc_einval();
    }
    // SAFETY: bionic's contract is a pointer to a 56-byte pthread_attr_t.
    let a = unsafe { &mut *(attr as *mut BionicAttr) };
    // Unlike every other attr function, this one *writes* the attr rather
    // than reading one the caller built -- so any existing backing is
    // replaced, the same posture `mutex_init`/`cond_init` take on an
    // explicit re-init, rather than reused.
    unsafe { destroy_backing(&a.state, &a.real_lo, &a.real_hi, pthread_attr_destroy) };
    a.state.store(UNINIT, Ordering::Release);
    let mut result: c_int = 0;
    let backing = unsafe {
        resolve(&a.state, &a.real_lo, &a.real_hi, |p| {
            result = pthread_getattr_np(thread, p);
        })
    };
    if backing.is_null() {
        libc_einval()
    } else {
        result
    }
}

/// Resolve a `pthread_attr_t*` handed to `pthread_create` to its real glibc
/// backing object -- exported for `native/thread_trace.cpp`'s
/// `cordial_pthread_create`, which otherwise forwards `attr` straight to the
/// host's own `pthread_create`. Correct on x86_64, where bionic's and
/// glibc's `pthread_attr_t` agree in size and this function does not even
/// exist; wrong on aarch64, where that would hand glibc a 56-byte object it
/// reads as its own 64-byte one -- the same overrun class `BionicMutex`
/// exists to stop for the hottest lock, one call site later.
///
/// A null `attr` (by far the common case -- most `pthread_create` calls pass
/// none) is returned unchanged rather than resolved into anything, matching
/// glibc's own "use the default attributes" reading of null. A resolution
/// failure also falls back to the original pointer rather than null: neither
/// is right for the anomalous state that would cause it, and this at least
/// reproduces the pre-fix behaviour rather than guaranteeing a crash on top
/// of whatever corrupted the attr.
#[cfg(target_arch = "aarch64")]
#[no_mangle]
pub extern "C" fn cordial_pthread_attr_real(attr: *const c_void) -> *const c_void {
    if attr.is_null() {
        return attr;
    }
    // SAFETY: as `attr_backing` above.
    match unsafe { attr_backing(attr) } {
        Some(backing) => backing,
        None => attr,
    }
}

// ------------------------------------------------- once, and thread-local keys

// glibc's. Renamed so the forwarding wrappers below can carry bionic's names.
extern "C" {
    #[link_name = "pthread_once"]
    fn host_pthread_once(control: *mut c_int, init_routine: Option<extern "C" fn()>) -> c_int;
    #[link_name = "pthread_key_create"]
    fn host_pthread_key_create(
        key: *mut c_uint,
        destructor: Option<extern "C" fn(*mut c_void)>,
    ) -> c_int;
    #[link_name = "pthread_key_delete"]
    fn host_pthread_key_delete(key: c_uint) -> c_int;
    #[link_name = "pthread_getspecific"]
    fn host_pthread_getspecific(key: c_uint) -> *mut c_void;
    #[link_name = "pthread_setspecific"]
    fn host_pthread_setspecific(key: c_uint, value: *const c_void) -> c_int;
}

/// `pthread_once`, forwarded to the host's.
///
/// These five were generated stubs until now, and a generated stub returns 0.
/// For most symbols that is a harmless placeholder; for these it is the lie
/// AGENTS.md forbids, in its worst form — the caller is told it *succeeded*.
/// `pthread_once` returning 0 means "your initialiser ran", so whatever it was
/// meant to set up is uninitialised and the next access faults with no visible
/// relationship to this call; `pthread_getspecific` returning 0 is a NULL the
/// caller dereferences on the next line. `cordial-run --lib-dir DIR` without
/// `--host-libc` segfaulted at exit 139 with `[stub] pthread_once` and
/// `[stub] pthread_getspecific` as the last two lines before the core dump.
///
/// Compiling bionic's own implementations instead is right for exactly one of
/// the five, which is why none of them does it. `pthread_key.cpp` reaches
/// thread-specific data through `__get_bionic_tls()`, which is
/// `__get_tls()[TLS_SLOT_BIONIC_TLS]` — a pointer to bionic's own thread
/// structure, hanging off the thread pointer of a thread bionic created. Every
/// thread in this process belongs to the host's libc, so that slot holds
/// whatever glibc keeps at that offset and the load reads the wrong memory.
/// That is worse than the stub it replaced, because it would appear to work.
/// `pthread_once.cpp` is the exception — a compare-exchange loop on the
/// caller's own `int` plus a futex, touching no thread structure at all — so it
/// could be ported standalone. Forwarding costs less and behaves the same.
///
/// Forwarding is safe *here* for a reason that does not generalise, and reading
/// it as a general licence is how `struct stat` and `sigset_t` would get passed
/// through next. It is safe only where the argument is laid out identically in
/// both libcs. `pthread_once_t` is `int` in both, 4 bytes against 4, and both
/// spell `PTHREAD_ONCE_INIT` as 0 — so a bionic once-control that was
/// statically initialised and never passed to bionic's implementation already
/// *is* a valid glibc one. `pthread_key_t` is 4 bytes in both and is opaque to
/// the caller. Compare `sem_t` at the top of this file, 16 against 32, where
/// the same forwarding would write past the end of the object.
///
/// One difference forwarding does not hide, and it is **INFERRED** — nothing
/// has been observed depending on it. bionic sets `KEY_VALID_FLAG`, bit 31, in
/// every key it hands out, so a bionic key is always a negative `int`; glibc's
/// are small non-negative ones and the first is 0. Code that treats key 0 as
/// "no key allocated" would be wrong here in a way it never was on Android.
///
/// # Safety
///
/// `control` must point at 4 bytes of storage bionic's `pthread_once_t`
/// occupies -- a statically- or `PTHREAD_ONCE_INIT`-initialised once-control
/// that has never been passed to bionic's own implementation, per the
/// comment above.
pub unsafe extern "C" fn once(control: *mut c_int, init_routine: Option<extern "C" fn()>) -> c_int {
    if control.is_null() {
        return libc_einval();
    }
    // SAFETY: `control` points at 4 bytes in both libcs, and a bionic
    // once-control that has never been passed to bionic's implementation holds
    // a state glibc's understands. `init_routine` is a plain `void (*)(void)`.
    unsafe { host_pthread_once(control, init_routine) }
}

/// `pthread_key_create`. bionic's `pthread_key_t` is signed, glibc's is not,
/// hence the local rather than a cast of the caller's pointer — and nothing is
/// written back unless the host says it succeeded, which is bionic's contract.
///
/// # Safety
///
/// `key` must point at 4 bytes of writable storage for the caller's
/// `pthread_key_t`.
pub unsafe extern "C" fn key_create(
    key: *mut c_int,
    destructor: Option<extern "C" fn(*mut c_void)>,
) -> c_int {
    if key.is_null() {
        return libc_einval();
    }
    let mut host_key: c_uint = 0;
    // SAFETY: `host_key` is a live 4-byte slot; the destructor is passed through
    // untouched and glibc calls it on the same thread bionic would have.
    let rc = unsafe { host_pthread_key_create(&mut host_key, destructor) };
    if rc == 0 {
        // SAFETY: the caller's `pthread_key_t*`, 4 bytes in both libcs.
        unsafe { *key = host_key as c_int };
    }
    rc
}

pub extern "C" fn key_delete(key: c_int) -> c_int {
    // SAFETY: a key is an opaque scalar; an invalid one is rejected by glibc.
    unsafe { host_pthread_key_delete(key as c_uint) }
}

pub extern "C" fn getspecific(key: c_int) -> *mut c_void {
    // SAFETY: as above. A key never created returns null, as bionic's does.
    unsafe { host_pthread_getspecific(key as c_uint) }
}

/// # Safety
///
/// `value` is stored and handed back by [`getspecific`], never dereferenced
/// here, so this has no requirement on it beyond being a value the caller
/// intends to get back; `key` is an opaque scalar glibc rejects if invalid.
pub unsafe extern "C" fn setspecific(key: c_int, value: *const c_void) -> c_int {
    // SAFETY: as above. `value` is stored, never dereferenced.
    unsafe { host_pthread_setspecific(key as c_uint, value) }
}

fn libc_einval() -> c_int {
    22 // EINVAL, identical in both libcs
}

/// `CORDIAL_COND_PASSTHROUGH=1`: leave `pthread_cond_*` to the host on x86_64.
///
/// **A diagnostic switch, off by default, and not a fix.** The condition
/// variable wrappers below put a heap object behind every engine `pthread_cond_t`
/// and add a lazy-init state machine in front of glibc's. The size table above
/// says the two libcs agree on the type (48 bytes, all-zero static initialiser),
/// so the wrapper was never needed on this architecture, only harmless by
/// argument -- and the startup freeze is a lost-wakeup-shaped wait. This makes
/// "is the wrapper in the way" a same-binary A/B rather than a theory. `sem_*`
/// stays wrapped either way: `sem_t` really does differ (16 against 32).
/// Only meaningful with `--host-libc`, which is what resolves the symbols this
/// stops overriding; without it they would fall to stubs.
fn cond_passthrough() -> bool {
    cfg!(target_arch = "x86_64")
        && std::env::var_os("CORDIAL_COND_PASSTHROUGH").is_some_and(|v| v == "1")
}

/// Everything this module replaces.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    overrides_with(cond_passthrough())
}

fn overrides_with(bypass_cond: bool) -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    // `mut` is only needed for the aarch64 extension below; unused on x86_64.
    #[allow(unused_mut)]
    let mut v = vec![
        f!("pthread_cond_init", cond_init),
        f!("pthread_cond_destroy", cond_destroy),
        f!("pthread_cond_wait", cond_wait),
        f!("pthread_cond_timedwait", cond_timedwait),
        f!("pthread_cond_signal", cond_signal),
        f!("pthread_cond_broadcast", cond_broadcast),
        f!("sem_init", semaphore_init),
        f!("sem_destroy", semaphore_destroy),
        f!("sem_post", semaphore_post),
        f!("sem_wait", semaphore_wait),
        f!("sem_trywait", semaphore_trywait),
        // Forwarded, not wrapped. These are here so `--lib-dir` alone does not
        // need `--host-libc` for them; see `once` for why forwarding is right
        // for these five and wrong for the two above.
        f!("pthread_once", once),
        f!("pthread_key_create", key_create),
        f!("pthread_key_delete", key_delete),
        f!("pthread_getspecific", getspecific),
        f!("pthread_setspecific", setspecific),
    ];
    // `pthread_mutex_t` and `pthread_attr_t` need translating only on
    // aarch64 -- see the module doc comment's table. On x86_64 the two libcs
    // agree in size and these symbols are left to resolve straight from the
    // host, exactly as before this fix.
    #[cfg(target_arch = "aarch64")]
    v.extend([
        f!("pthread_mutex_init", mutex_init),
        f!("pthread_mutex_destroy", mutex_destroy),
        f!("pthread_mutex_lock", mutex_lock),
        f!("pthread_mutex_trylock", mutex_trylock),
        f!("pthread_mutex_unlock", mutex_unlock),
        f!("pthread_mutex_timedlock", mutex_timedlock),
        f!("pthread_attr_init", attr_init),
        f!("pthread_attr_destroy", attr_destroy),
        f!("pthread_attr_setstacksize", attr_setstacksize),
        f!("pthread_attr_getstacksize", attr_getstacksize),
        f!("pthread_attr_setdetachstate", attr_setdetachstate),
        f!("pthread_attr_getdetachstate", attr_getdetachstate),
        f!("pthread_attr_setguardsize", attr_setguardsize),
        f!("pthread_attr_getguardsize", attr_getguardsize),
        f!("pthread_attr_setschedparam", attr_setschedparam),
        f!("pthread_attr_getschedparam", attr_getschedparam),
        f!("pthread_attr_setstack", attr_setstack),
        f!("pthread_attr_getstack", attr_getstack),
        f!("pthread_getattr_np", getattr_np),
    ]);
    if bypass_cond {
        v.retain(|(name, _)| !name.starts_with("pthread_cond_"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers_fit_inside_the_bionic_objects() {
        // A wrapper is an overlay on storage the caller allocated to bionic's
        // size, so it must never be larger than bionic's type. `sem_t` is 16
        // and the overlay uses all of it; `pthread_cond_t` is 48 and the
        // overlay spans all 48, of which it reads and writes the first 12.
        assert!(std::mem::size_of::<BionicCond>() <= 48);
        assert_eq!(std::mem::size_of::<BionicSem>(), 16);
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(std::mem::size_of::<BionicMutex>(), 40);
            assert_eq!(std::mem::size_of::<BionicAttr>(), 56);
        }
    }

    #[test]
    fn cond_passthrough_drops_exactly_the_cond_overrides() {
        let names = |bypass| -> Vec<&'static str> {
            overrides_with(bypass).into_iter().map(|(n, _)| n).collect()
        };
        let wrapped = names(false);
        let bypassed = names(true);
        assert!(wrapped.contains(&"pthread_cond_wait"));
        assert!(wrapped.contains(&"pthread_cond_broadcast"));
        assert!(!bypassed.iter().any(|n| n.starts_with("pthread_cond_")));
        // Everything else, sem_* above all, is untouched by the switch.
        assert!(bypassed.contains(&"sem_wait"));
        assert!(bypassed.contains(&"pthread_once"));
        let kept: Vec<_> = wrapped.iter().filter(|n| !n.starts_with("pthread_cond_")).collect();
        assert_eq!(kept.len(), bypassed.len());
    }

    #[test]
    fn statically_initialised_cond_works() {
        // bionic's PTHREAD_COND_INITIALIZER is all zeroes; signalling one that
        // was never explicitly initialised must still work.
        // u64 storage is more aligned than the bionic object requires. The
        // wrapper itself uses only four-byte atomics because bionic condition
        // variables are allowed to start at four-byte alignment. Six words, not
        // four: the overlay is 48 bytes, and a reference to it over 32 bytes of
        // storage is undefined behaviour even though only 12 are touched.
        let mut storage = [0u64; 6];
        let cond = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(cond_signal(cond), 0);
        assert_eq!(cond_broadcast(cond), 0);
        assert_eq!(cond_destroy(cond), 0);
    }

    /// The first use of a statically-initialised condition variable is a race
    /// between whoever waits and whoever signals, and the wrapper's lazy
    /// initialisation sits exactly there. A lost wakeup on that first use would
    /// look like a wait that nothing ever ends, so this hammers only the first
    /// use: a fresh, all-zero cond and mutex per iteration, several threads at
    /// once, signal issued after the lock is dropped (legal, and the ordering
    /// that leaves the most room for a wake to land before the wait begins).
    ///
    /// x86_64 only: it locks with the host's `pthread_mutex_t`, which is the
    /// bionic layout there and not on aarch64.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn first_use_race_never_loses_a_wakeup() {
        use std::sync::atomic::AtomicU32;
        use std::sync::mpsc;
        use std::time::Duration;

        extern "C" {
            fn pthread_mutex_lock(m: *mut c_void) -> c_int;
            fn pthread_mutex_unlock(m: *mut c_void) -> c_int;
        }
        #[repr(C, align(16))]
        struct Fresh {
            cond: [u64; 6],
            mutex: [u64; 5],
            flag: AtomicU32,
        }

        const WORKERS: usize = 4;
        const ROUNDS: usize = 3000;
        let (tx, rx) = mpsc::channel::<usize>();
        for w in 0..WORKERS {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for round in 0..ROUNDS {
                    let fresh = Box::into_raw(Box::new(Fresh {
                        cond: [0; 6],
                        mutex: [0; 5],
                        flag: AtomicU32::new(0),
                    }));
                    let addr = fresh as usize;
                    let waiter = std::thread::spawn(move || {
                        let f = addr as *mut Fresh;
                        // SAFETY: `f` outlives both threads; the join below orders the free.
                        unsafe {
                            let m = std::ptr::addr_of_mut!((*f).mutex) as *mut c_void;
                            let c = std::ptr::addr_of_mut!((*f).cond) as *mut c_void;
                            pthread_mutex_lock(m);
                            while (*f).flag.load(Ordering::Acquire) == 0 {
                                cond_wait(c, m);
                            }
                            pthread_mutex_unlock(m);
                        }
                    });
                    // Vary who gets there first, including simultaneously.
                    for _ in 0..(round % 7) * 40 {
                        std::hint::spin_loop();
                    }
                    // SAFETY: as above.
                    unsafe {
                        let m = std::ptr::addr_of_mut!((*fresh).mutex) as *mut c_void;
                        let c = std::ptr::addr_of_mut!((*fresh).cond) as *mut c_void;
                        pthread_mutex_lock(m);
                        (*fresh).flag.store(1, Ordering::Release);
                        pthread_mutex_unlock(m);
                        if round % 2 == 0 {
                            cond_signal(c);
                        } else {
                            cond_broadcast(c);
                        }
                    }
                    waiter.join().unwrap();
                    // SAFETY: both users are done.
                    unsafe {
                        cond_destroy(std::ptr::addr_of_mut!((*fresh).cond) as *mut c_void);
                        drop(Box::from_raw(fresh));
                    }
                }
                tx.send(w).unwrap();
            });
        }
        drop(tx);
        for _ in 0..WORKERS {
            rx.recv_timeout(Duration::from_secs(60))
                .expect("a worker never finished: a wakeup was lost on a cond's first use");
        }
    }

    #[test]
    fn init_destroy_roundtrip_does_not_leak_state() {
        let mut storage = [0u64; 6];
        let cond = storage.as_mut_ptr() as *mut c_void;
        // SAFETY: `cond` is a live, correctly-sized `pthread_cond_t` on this
        // thread's stack, per `cond_init`/`cond_destroy`'s own contracts.
        unsafe {
            assert_eq!(cond_init(cond, std::ptr::null()), 0);
            assert_eq!(cond_destroy(cond), 0);
            // Destroyed wrappers return to the zero state, so they can be reused.
            assert_eq!(cond_init(cond, std::ptr::null()), 0);
            assert_eq!(cond_destroy(cond), 0);
        }
    }

    #[test]
    fn once_runs_the_initialiser_exactly_once() {
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        extern "C" fn init() {
            RUNS.fetch_add(1, Ordering::SeqCst);
        }
        // bionic's PTHREAD_ONCE_INIT is 0, and so is glibc's. A control that
        // was only ever statically initialised is valid for both.
        let mut control: c_int = 0;
        // SAFETY: `control` is a live, statically-zeroed `pthread_once_t` on
        // this thread's stack, per `once`'s own contract.
        unsafe {
            assert_eq!(once(&mut control, Some(init)), 0);
            assert_eq!(once(&mut control, Some(init)), 0);
        }
        assert_eq!(RUNS.load(Ordering::SeqCst), 1);

        // The control, not the routine, is what remembers. A second control
        // runs it again — otherwise the count above proves nothing.
        let mut second: c_int = 0;
        // SAFETY: as above.
        unsafe {
            assert_eq!(once(&mut second, Some(init)), 0);
        }
        assert_eq!(RUNS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn thread_specific_data_round_trips() {
        let mut key: c_int = -1;
        // SAFETY: `key` is a live 4-byte slot on this thread's stack, per
        // `key_create`/`setspecific`'s own contracts.
        unsafe {
            assert_eq!(key_create(&mut key, None), 0);
        }
        // A key with nothing stored reads as null, which is what the caller
        // that used to get a stubbed 0 was entitled to expect.
        assert!(getspecific(key).is_null());
        let value = 0xC0FFEEusize as *const c_void;
        // SAFETY: as above.
        unsafe {
            assert_eq!(setspecific(key, value), 0);
        }
        assert_eq!(getspecific(key), value as *mut c_void);
        assert_eq!(key_delete(key), 0);
    }

    #[test]
    fn thread_specific_data_is_per_thread() {
        // The property bionic's own implementation would have got wrong here:
        // it reads the slot table hanging off the thread pointer, which for a
        // host-created thread is glibc's.
        let mut key: c_int = -1;
        // SAFETY: as `thread_specific_data_round_trips` above.
        unsafe {
            assert_eq!(key_create(&mut key, None), 0);
            assert_eq!(setspecific(key, 1 as *const c_void), 0);
        }

        let elsewhere = key;
        let seen = std::thread::spawn(move || getspecific(elsewhere) as usize)
            .join()
            .unwrap();
        assert_eq!(seen, 0, "a fresh thread must not see this thread's value");
        assert_eq!(getspecific(key) as usize, 1);
        assert_eq!(key_delete(key), 0);
    }

    #[test]
    fn null_arguments_are_refused_rather_than_dereferenced() {
        // SAFETY: both refuse a null argument before doing anything with it,
        // which is exactly what this test asserts.
        unsafe {
            assert_eq!(once(std::ptr::null_mut(), None), libc_einval());
            assert_eq!(key_create(std::ptr::null_mut(), None), libc_einval());
        }
    }

    #[test]
    fn semaphore_counts() {
        let mut storage = [0u64; 2];
        let sem = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(semaphore_init(sem, 0, 1), 0);
        assert_eq!(semaphore_wait(sem), 0); // consumes the one permit
        assert_ne!(semaphore_trywait(sem), 0); // none left
        assert_eq!(semaphore_post(sem), 0);
        assert_eq!(semaphore_trywait(sem), 0);
        assert_eq!(semaphore_destroy(sem), 0);
    }

    // ---------------------------------------------------- aarch64-only: mutexes

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn statically_initialised_normal_mutex_locks_and_unlocks() {
        // bionic's PTHREAD_MUTEX_INITIALIZER is all zeroes; locking one that
        // was never explicitly initialised must still work, and must behave
        // as a normal (non-recursive) mutex.
        let mut storage = [0u64; 5]; // 40 bytes
        let m = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(mutex_lock(m), 0);
        assert_eq!(mutex_unlock(m), 0);
        assert_eq!(mutex_destroy(m), 0);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn statically_initialised_recursive_mutex_can_be_relocked_by_its_owner() {
        // PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP's raw value:
        // (PTHREAD_MUTEX_RECURSIVE & 3) << MUTEX_TYPE_SHIFT, i.e. 1 << 14 --
        // see `static_mutex_kind`'s own comment for where these numbers come
        // from. A mutex declared with this macro and never passed to
        // `pthread_mutex_init` must still behave as recursive on its very
        // first lock.
        const RECURSIVE_INITIALIZER: u64 = 1 << 14;
        let mut storage = [0u64; 5];
        storage[0] = RECURSIVE_INITIALIZER;
        let m = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(mutex_lock(m), 0);
        assert_eq!(
            mutex_trylock(m),
            0,
            "a recursive mutex must let its own owner lock it again"
        );
        assert_eq!(mutex_unlock(m), 0);
        assert_eq!(mutex_unlock(m), 0);
        assert_eq!(mutex_destroy(m), 0);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn statically_initialised_errorcheck_mutex_refuses_a_self_relock() {
        // PTHREAD_ERRORCHECK_MUTEX_INITIALIZER_NP: (PTHREAD_MUTEX_ERRORCHECK
        // & 3) << 14, i.e. 2 << 14. The opposite property from the recursive
        // case above -- proof that the two static patterns are not being
        // confused for one another.
        const ERRORCHECK_INITIALIZER: u64 = 2 << 14;
        let mut storage = [0u64; 5];
        storage[0] = ERRORCHECK_INITIALIZER;
        let m = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(mutex_lock(m), 0);
        assert_ne!(
            mutex_lock(m),
            0,
            "an errorcheck mutex must refuse a self-relock rather than deadlock or succeed"
        );
        assert_eq!(mutex_unlock(m), 0);
        assert_eq!(mutex_destroy(m), 0);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn mutex_lock_is_exclusive_under_contention() {
        // If the lock were not truly exclusive -- the whole point of the
        // wrapper existing -- concurrent threads racing on this
        // load-then-store would lose increments and the final count would
        // come out short.
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        const THREADS: usize = 8;
        const ITERS: usize = 2000;

        let mut storage = [0u64; 5];
        let m = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(mutex_init(m, std::ptr::null()), 0);
        let address = m as usize;

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                std::thread::spawn(move || {
                    let mutex = address as *mut c_void;
                    for _ in 0..ITERS {
                        assert_eq!(mutex_lock(mutex), 0);
                        let v = COUNTER.load(Ordering::Relaxed);
                        COUNTER.store(v + 1, Ordering::Relaxed);
                        assert_eq!(mutex_unlock(mutex), 0);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(COUNTER.load(Ordering::SeqCst), (THREADS * ITERS) as u32);
        assert_eq!(mutex_destroy(m), 0);
    }

    // ------------------------------------------------------- aarch64-only: attrs

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn attr_setstacksize_round_trips_through_the_getter() {
        let mut storage = [0u64; 7]; // 56 bytes
        let attr = storage.as_mut_ptr() as *mut c_void;
        assert_eq!(attr_init(attr), 0);
        let requested: usize = 1 << 20;
        assert_eq!(attr_setstacksize(attr, requested), 0);
        let mut got: usize = 0;
        assert_eq!(attr_getstacksize(attr, &mut got), 0);
        assert_eq!(got, requested);
        assert_eq!(attr_destroy(attr), 0);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn attr_round_trips_a_set_stack_size_through_pthread_create() {
        // Exercises the real path an engine call takes: this file's
        // `attr_init`/`attr_setstacksize`, then `native/thread_trace.cpp`'s
        // `cordial_pthread_create` (which calls back into
        // `cordial_pthread_attr_real` on aarch64 before forwarding to the
        // host's own `pthread_create`), then the new thread reading its own
        // stack size back via `getattr_np`/`attr_getstacksize` -- the same
        // two functions Roblox's engine would call, on the same object,
        // through the same wrapper.
        extern "C" {
            fn cordial_pthread_create(
                thread: *mut c_ulong,
                attr: *const c_void,
                start_routine: extern "C" fn(*mut c_void) -> *mut c_void,
                arg: *mut c_void,
            ) -> c_int;
            fn pthread_join(thread: c_ulong, retval: *mut *mut c_void) -> c_int;
            fn pthread_self() -> c_ulong;
        }

        static SEEN_STACK: AtomicU32 = AtomicU32::new(0);
        // A whole page above PTHREAD_STACK_MIN on any Linux, and page-aligned
        // so nothing rounds it before the read-back.
        const REQUESTED_STACK: usize = 1 << 20;

        extern "C" fn start(_: *mut c_void) -> *mut c_void {
            let mut self_attr_storage = [0u64; 7];
            let self_attr = self_attr_storage.as_mut_ptr() as *mut c_void;
            // SAFETY: `pthread_self` takes no arguments and returns the
            // calling thread's own handle.
            let me = unsafe { pthread_self() };
            assert_eq!(getattr_np(me, self_attr), 0);
            let mut size: usize = 0;
            assert_eq!(attr_getstacksize(self_attr, &mut size), 0);
            SEEN_STACK.store(size as u32, Ordering::SeqCst);
            assert_eq!(attr_destroy(self_attr), 0);
            std::ptr::null_mut()
        }

        let mut attr_storage = [0u64; 7];
        let attr = attr_storage.as_mut_ptr() as *mut c_void;
        assert_eq!(attr_init(attr), 0);
        assert_eq!(attr_setstacksize(attr, REQUESTED_STACK), 0);

        let mut thread: c_ulong = 0;
        // SAFETY: `attr` is a live, initialised BionicAttr; `start` matches
        // the expected signature and returns null.
        let rc = unsafe {
            cordial_pthread_create(&mut thread, attr, start, std::ptr::null_mut())
        };
        assert_eq!(rc, 0, "pthread_create with a translated attr must succeed");
        // SAFETY: `thread` came from the `cordial_pthread_create` call above.
        assert_eq!(unsafe { pthread_join(thread, std::ptr::null_mut()) }, 0);

        assert_eq!(
            SEEN_STACK.load(Ordering::SeqCst) as usize,
            REQUESTED_STACK,
            "the new thread must see the stack size that was set on the attr \
             it was created with, round-tripped through pthread_create and \
             pthread_getattr_np"
        );
        assert_eq!(attr_destroy(attr), 0);
    }
}
