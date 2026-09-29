# ADR-040: The engine already runs mimalloc, so there is no allocator to switch

**Status:** accepted
**Supersedes:** nothing
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-034](ADR-034-symbol-resolution-asks-the-library.md)

## Decision

Cordial does not add a `CORDIAL_ENGINE_ALLOCATOR=mimalloc|glibc` switch, and
`crates/cordial-runtime/src/mimalloc_lib.rs`'s virtual `libmimalloc.so` is left
as-is rather than wired to anything. Both were the shape a prior brief for this
work assumed; both rest on a premise this change measured to be false.

## Why

**The premise was that the engine's `malloc` family resolves to glibc through
`symtab.rs`.** It does not, and it cannot, because the engine never asks
`symtab.rs` to resolve `malloc`, `free`, `calloc`, `realloc`,
`reallocarray`, `memalign`, `posix_memalign`, `aligned_alloc`, `valloc`,
`pvalloc` or `malloc_usable_size` in the first place:

```
readelf --dyn-syms -W ~/.cache/cordial/lib/x86_64/libroblox.so \
  | awk '$7=="UND" {print $8}' | sed 's/@.*//' | sort -u \
  | grep -E '^(malloc|free|calloc|realloc|reallocarray|memalign|posix_memalign|aligned_alloc|valloc|pvalloc|malloc_usable_size)$'
```

returns nothing. None of these names appear anywhere in the dynamic symbol
table — not as an import (`UND`), not as an export. That is not "the engine
resolves them from somewhere else"; it means these calls never cross the
`.so`'s own boundary at all. Nothing external can be resolved into a call
that is never made externally.

**The reason is that the engine already carries its own allocator, statically
linked, and it is mimalloc.** `libroblox.so`'s read-only data holds mimalloc's
own source strings verbatim — `"environment option \"mimalloc_%s\" is
deprecated -- use \"mimalloc_%s\" instead."`, the full `mi_option_*` name
table, the literal word `Mimalloc` — which is not something a binary picks up
by accident; it is mimalloc's `options.c` compiled in. Running Cordial and
reading the engine's own log confirms it fires, not merely that the strings
exist:

```
$ grep -i mimalloc appData/logs/*_Player_*.log
...,Critical [DFLog::Mimalloc] Mimalloc integration detected, settings:
...,Critical [DFLog::Mimalloc] mi_option_show_errors=1
...,Info     [DFLog::Mimalloc] mi_option_show_stats=0
...,Info     [DFLog::Mimalloc] mi_option_verbose=0
...(37 more mi_option_* lines)
```

Measured 2026-09-29, `--host-libc --game-activity`, signed-out landing UI, the
prebuilt `target-toolbox/release/cordial-run` at commit `06da8306`. This line
prints at t≈1.15s into every run this session tried, unconditionally — it is
not gated on anything Cordial does or does not provide, which is consistent
with a statically-linked integration that runs regardless of the host. The
`overrides()` doc comment in `mimalloc_lib.rs` already recorded that a direct
`dlopen`/`dlsym` trace saw zero requests naming mimalloc; this is the other
half of that finding. The engine was never going to ask for a virtual
`libmimalloc.so`, not because Cordial failed to offer it convincingly, but
because the engine had no gap to fill — it brought its own copy.

**So "mimalloc vs glibc for the engine's own allocations" is not an available
comparison.** There is no glibc arm: glibc's `malloc` was never the engine's
allocator on this build, on any Cordial run, ever. The measurement this task
asked for — CPU%, RSS and perf `--sort dso` malloc/free share, compared
between arms — was not run, because there is no second arm to run it against.
Building the `CORDIAL_ENGINE_ALLOCATOR` switch as specified would have shipped
a setting that changes nothing the moment it is not `Display`... which is
exactly the "stub that reports success and does nothing" shape AGENTS.md
opens with, one layer further out: a *setting*, not a stub, but the same
failure the moment somebody flips it expecting a difference.

## What this does not settle

**Whether the mimalloc build inside `libroblox.so` is well-tuned for this
host is untested.** Mimalloc reads its own environment (`MIMALLOC_*`, per the
deprecated-spelling message above, which implies an even older `mimalloc_*`
form) for configuration, and nothing stops a future change from setting those
variables before `cordial-run` starts the engine, the same way `CORDIAL_*`
variables are set today. That is a real, ADR-001-compliant lever — it is
process environment, not memory patching, exactly the class of thing
`graphics.rs` and `flags.rs` already do — and it was not explored here for
lack of a baseline to tune against; only the presence of the integration was
established this session, not its options' effect on this hardware.

## A real cross-allocator danger this did surface, separate from the above

Auditing the "engine frees what a host libc call allocated" list this task
named turned up a live instance, already found and fixed once, and two more
candidates, which have since been guarded (below).

**That Roblox statically links mimalloc was not this ADR's discovery — it was
already in `docs/NEXT.md`'s "Solved, for reference" table**, one line, from the
`realpath` investigation below: "`realpath(path, NULL)` allocates with the host
allocator; Roblox statically links mimalloc and freed a pointer its arena
table never registered." What this ADR adds is the connection from that one
fixed crash to the broader claim a task brief made independently of it — that
the engine's allocator is something Cordial could still pick between mimalloc
and glibc for — and the fresh verification (the readelf import list, and a
2026-09-29 log capture of the detection line itself) that settles it rather
than resting on the one prior incident.

`native/system_paths.cpp`'s `s_realpath` documents exactly this failure,
caught live under lldb: `realpath(path, NULL)` resolved to the host's real
`realpath`, which `malloc`s its return buffer from glibc; the engine's own
allocator "indexes a table keyed by the pointer's own address to find the
arena that owns it," a host-`malloc`'d pointer was never registered in that
table, and the next dereference faulted — `rax=0x0, rcx=0xe000`, a
segment-map miss for foreign memory. That description is mimalloc's own
segment/page lookup, read back before this ADR had confirmed by name that
mimalloc is what the engine runs. The fix already shipped: `s_realpath` never
produces the host allocation for the `NULL`-buffer form; it resolves into a
stack buffer for the trace log and reports `ENOTSUP`, which is a real,
if degraded, POSIX outcome every caller of that form must already handle.

**`vasprintf` and `getcwd` are imported by this same build** —

```
readelf --dyn-syms -W libroblox.so | awk '$7=="UND"{print $8}' | sort -u \
  | grep -E '^(getcwd|vasprintf)$'
```

— and, when this was written, neither had a Cordial override in `crate::bionic`. `symtab.rs::resolve`
sends any `Class::Generic` symbol with no override straight to the host's real
libc.so.6 whenever `--host-libc` is set, which is the flag AGENTS.md's own
example invocation uses. `vasprintf` always allocates its return buffer —
there is no bounded-buffer form the way `getcwd(buf, size)` is one — so every
call the engine makes through it, if resolved from the host, hands the engine
a pointer for glibc's allocator's bookkeeping that its own `free()` cannot
see. `getcwd(NULL, 0)` is the same shape as `realpath(path, NULL)` exactly:
safe when the engine supplies its own buffer, and the same latent fault as
`s_realpath`'s when it does not.

**That was the state when this ADR was written, and it has since been audited
and guarded** (2026-09-29, `native/foreign_alloc.cpp`). Both resolved to glibc:
the engine's own GOT was read back under gdb and pointed at `libc.so.6`'s
`vasprintf` and `getcwd`. Both now resolve to Cordial shims that refuse the
allocating form, the way `s_realpath` does, and report failure with `ENOMEM`,
the errno each is documented to set when it cannot allocate. `getcwd` with a
caller buffer is forwarded untouched.

**Neither refusal was seen to fire, so both rest on INFERRED grounds.** Two
signed-out runs, 40 s and 110 s, covering startup, the landing page and the
sign-in form, with gdb breakpoints on the host entry points: the engine made
four `getcwd` calls, all with a 128-byte caller buffer, and no `vasprintf`
call. `realpath` was not called by the engine either, so even the confirmed
case did not recur in those runs. Anything in a game session is unexercised.
The first eight refusals of each are logged to stderr as `[alloc] ...`, so a
run that reaches one says so.

Every other import that returns or takes memory was checked and needs nothing:

- `getaddrinfo`/`freeaddrinfo` are both Cordial shims (`netdb_compat.cpp`) that
  copy the result, so the engine never holds a glibc block.
- `newlocale`/`freelocale`, `opendir`/`closedir`, `fdopen`/`fopen`/`fclose` and
  `pthread_attr_init`/`pthread_getattr_np`/`pthread_attr_destroy` all resolve to
  the host on both halves. `pthread_getattr_np` allocates a cpuset that only
  `pthread_attr_destroy` frees; the engine made 38 calls and 38 adjacent
  destroys, so nothing leaks.
- `sscanf`/`fscanf`/`vsscanf` allocate for `%m`. 34,049 `fscanf` and 1,546
  `sscanf` calls in the 110 s run, none with one, and the binary has no `%m`
  format string. Not guarded: wrapping a variadic on that path costs more than
  the risk.
- Not imported at all, so unreachable except through `dlsym`, and the engine
  looked up only `getauxval` from libc: `asprintf`, `strdup`, `strndup`,
  `wcsdup`, `getline`, `getdelim`, `open_memstream`, `scandir`, `tempnam`,
  `canonicalize_file_name`, `get_current_dir_name`, `backtrace_symbols`.
- The reverse direction, engine memory freed by glibc, has no importer: `putenv`
  and `setenv` are not imported, `setvbuf` was never called, and glibc does not
  free a caller-supplied stdio or thread-stack buffer.

## What would change this

If a future Roblox build drops its statically-linked mimalloc — the log stops
printing `[DFLog::Mimalloc] Mimalloc integration detected` — the premise this
ADR corrects would be worth re-examining, because at that point `malloc` might
genuinely become an external, resolvable import again.
