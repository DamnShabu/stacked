//! `libmimalloc.so`, backed by the vendored mimalloc rather than a stub.
//!
//! Roblox's engine narrates its own allocator choice: on real Android and on
//! Sober it logs `[DFLog::Mimalloc] Mimalloc integration detected, settings:`
//! followed by roughly forty `mi_option_*=value` lines. `libroblox.so` does not
//! link mimalloc — its own undefined-symbol table has zero `mi_`-prefixed
//! entries (`docs/analysis/undefined-symbols.tsv`) — so whatever asks for it
//! does so as a library the engine goes looking for at runtime, the same way it
//! goes looking for Vulkan rather than linking it. Sober answers that lookup by
//! shipping a real `libmimalloc.so` beside its binary; this does the same thing
//! by vendoring mimalloc into Cordial itself and registering it as a virtual
//! library, the same pattern `android::vulkan` uses.
//!
//! AGENTS.md is explicit that a stub must never lie about what it can do, and
//! that applies with extra force here: mimalloc's option getters are cheap to
//! fake convincingly (they are just an enum-indexed table), but an engine told
//! "mimalloc is here" that then keeps allocating through glibc via `libc.so`'s
//! ordinary `malloc` would run — just not through the allocator it believes it
//! configured, which is a worse failure than an honest absence because nothing
//! points back at the mismatch. That is why this links the real allocator
//! (`libmimalloc-sys`, vendoring upstream mimalloc's C source) rather than
//! writing a handful of functions that only return plausible numbers.
//!
//! Whether the engine ever calls any of this is a separate question from
//! whether it is provided — see the doc comment on `LIBRARY_NAME` below and
//! the session notes this change shipped with: a direct trace of every guest
//! `dlopen`/`dlsym` call recorded zero requests naming mimalloc, so linking it
//! in is necessary but was not, on its own, observed to be sufficient.
//!
//! **Now known why, rather than merely observed: the engine never needed to
//! ask.** [ADR-040](../../../../docs/adr/ADR-040-the-engine-already-runs-mimalloc.md)
//! establishes, by reading `libroblox.so`'s own strings and by running it and
//! reading its log, that mimalloc is statically linked *into the engine
//! itself* — `[DFLog::Mimalloc] Mimalloc integration detected` prints
//! unconditionally, roughly 1.15 s into every run, with no dependency on
//! anything Cordial provides. `malloc`, `free` and the rest of that family are
//! not even present in `libroblox.so`'s dynamic symbol table, as an import or
//! an export: the engine's C++ calls them directly against its own compiled-in
//! implementation, which is why nothing here was ever going to see a `dlopen`
//! for this file. This module is not wrong to exist — it is a faithful answer
//! to "what if the engine asks" — but the honest status is that there is
//! nothing left to ask for, and `CORDIAL_ENGINE_ALLOCATOR`-shaped feature that
//! would pick between "the engine's malloc" and "glibc" has no second option
//! to pick.

use std::ffi::c_void;

use libmimalloc_sys as mi;

/// The soname the engine would need to `dlopen` to find this. Not confirmed —
/// `INFERRED` from Sober shipping a file of exactly this name beside its
/// binary — because the guest was never observed asking for it by any name in
/// a Cordial run; see the module doc comment.
pub const LIBRARY_NAME: &str = "libmimalloc.so";

/// Symbols exported under `libmimalloc.so`.
///
/// This is not mimalloc's full surface (upstream has well over a hundred
/// entry points); it is the allocation primitives, the lifecycle hooks, and
/// the option accessors — `mi_option_get`/`mi_option_set` and friends — that
/// the ~forty `mi_option_*=value` lines in the real log imply the engine
/// queries by, rather than one exported symbol per option name (the
/// `mi_option_show_errors`-shaped strings sitting in `libroblox.so` are option
/// *names* for that log line, not symbols anything dlsyms — mimalloc's own
/// options are a C enum, not one function per option). Extend this list from
/// `libmimalloc-sys`'s `extended` feature if a future trace shows the engine
/// asking for something not covered here.
pub fn overrides() -> Vec<(&'static str, *mut c_void)> {
    macro_rules! f {
        ($name:literal, $fn:expr) => {
            ($name, $fn as *const () as *mut c_void)
        };
    }
    vec![
        // Presence probe. Real mimalloc's version encoding, e.g. 217 for 2.1.7.
        f!("mi_version", mi::mi_version),
        // Core allocation, the functions an integration actually allocates
        // through once it has decided mimalloc is present.
        f!("mi_malloc", mi::mi_malloc),
        f!("mi_zalloc", mi::mi_zalloc),
        f!("mi_calloc", mi::mi_calloc),
        f!("mi_realloc", mi::mi_realloc),
        f!("mi_free", mi::mi_free),
        f!("mi_malloc_aligned", mi::mi_malloc_aligned),
        f!("mi_zalloc_aligned", mi::mi_zalloc_aligned),
        f!("mi_realloc_aligned", mi::mi_realloc_aligned),
        f!("mi_usable_size", mi::mi_usable_size),
        f!("mi_strdup", mi::mi_strdup),
        // Lifecycle. Real Android's process/thread teardown calls these;
        // without them a thread that exits leaks its heap's segments back to
        // mimalloc's arena instead of returning them to the OS.
        f!("mi_process_init", mi::mi_process_init),
        f!("mi_thread_init", mi::mi_thread_init),
        f!("mi_thread_done", mi::mi_thread_done),
        f!("mi_collect", mi::mi_collect),
        // Options. `mi_option_get`/`mi_option_set` take the enum value as an
        // int, not a name — the ~forty `mi_option_*` lines in the real log are
        // this loop's own after-the-fact naming of each enum entry for the
        // reader, produced by Roblox's code, not by a symbol lookup per name.
        f!("mi_option_get", mi::mi_option_get),
        f!("mi_option_set", mi::mi_option_set),
        f!("mi_option_is_enabled", mi::mi_option_is_enabled),
        f!("mi_option_set_enabled", mi::mi_option_set_enabled),
        f!("mi_option_set_enabled_default", mi::mi_option_set_enabled_default),
        // Diagnostics. `mi_option_show_stats`/`mi_option_verbose` control
        // whether the engine ever calls these; wiring them up costs nothing.
        f!("mi_stats_reset", mi::mi_stats_reset),
        f!("mi_stats_print_out", mi::mi_stats_print_out),
    ]
}
