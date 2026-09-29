# Multi-architecture strategy

**Task B (§16.3). Status: decided.**

## Decision

**Execute natively when the host ABI matches an ABI the APK ships. Do not build a
translation layer.**

Task A established that Roblox ships a complete x86-64 Android build (see
[`findings.md`](findings.md) §1). Cordial therefore never translates machine code. This
is a build-flag and runtime-dispatch concern, not an architectural one.

| Host | APK ships a matching ABI | Strategy | Phase |
|---|---|---|---|
| x86-64 | `lib/x86_64/` — yes | Native execution + CPU *feature* emulation (`libbadcpu`) | 1 |
| ARM64 | `lib/arm64-v8a/` — yes | Native execution of `arm64-v8a`; no feature emulator needed | later |
| ARM64 | absent | Translation required | **not supported** |
| anything else | — | — | not supported |

## Consequences

**x86-64 is the only supported target for Phases 1–2.** Everything else is deferred until
the runtime actually launches Roblox on the primary target.

**ARM64 hosts are cheap in principle and expensive in practice.** The ABI is present in
the APK, so the loader, bionic shim, syscall translation and framework layer are all
architecture-agnostic in design. Treat ARM64 as a real port, not a compile flag, and do
not attempt it until x86-64 works. `libbadcpu` is not built on ARM64: it is an x86-64
instruction emulator and has no meaning there.

**The reason has changed, though, and the sentence above used to give the wrong one.** It
said each layer "contains architecture-specific code (TLS layout, relocation types,
syscall numbers, signal frame layout)". Inventoried on 2026-08-26: **ours contains none of
it.** `git grep -E '__x86_64__|__aarch64__|asm volatile|target_arch' -- crates native`
matches nothing across all 159 tracked files — no TLS handling, no relocation switch, no
syscall wrapper, no signal trampoline, no assembly. All of that is upstream bionic's, and
upstream already has it per-architecture: `third_party/mcpelauncher-linker/CMakeLists.txt`
carries an arm64 branch, the arch trees are checked out, and `GetTargetElfMachine()`
already returns `EM_AARCH64` on an aarch64 build, so a wrong-architecture library is
refused with a readable message for free.

**What is expensive is a thing this document did not anticipate: the page size.**
`third_party/mcpelauncher-linker/include/compat.h` defines `PAGE_SIZE` as a literal 4096
and force-includes it into every linker translation unit. Roughly half the hardware people
ask about is not 4K — Asahi on Apple silicon is 16K by hardware mandate, and recent
Raspberry Pi OS defaults the Pi 5 to a 16K kernel, while most postmarketOS phone SoCs and
Graviton are 4K. Upstream *does* detect this at run time and switch strategy, and the
strategy it switches to is the problem: it maps the whole reservation `PROT_READ |
PROT_WRITE | PROT_EXEC` and copies the ELF in by hand, never re-protecting it.

**That silently voids ADR-001's guarantee.** `patches/0001-map-engine-text-read-only.patch`
edits an `mmap64` the 16K path never reaches, so the patch is not broken by this — it is
bypassed, and 106 MB of engine text stays `rwxp` for the life of the process. ADR-001's
verification is also scoped to "Roblox's own x86-64 build"; the arm64-v8a build's
`DT_TEXTREL` status has never been looked at.

So the honest split is: **a 4K aarch64 host is plausibly a mechanical job, and a 16K one is
not** — and two of the three platforms people ask for are 16K. Android 15 also requires
16 KB-aligned native libraries, so an arm64-v8a `libroblox.so` will likely carry
`p_align = 16384` and take a different loader path from the x86-64 one *even on a 4K host*.
The port cannot assume 4K aarch64 behaves like x86-64.

**The cheapest next step needs no ARM hardware at all:** `readelf` an `arm64-v8a`
`libroblox.so` and read off its `DT_TEXTREL`, `p_align`, relocation counts and CPU feature
floor. That closes a real slice of the unknown for the price of one APK.

**No translation layer will be designed.** If a future host has no matching ABI, the
answer is "unsupported", not "write a JIT". Reopening this requires reopening Task A.

**No Quest/VR target.** The Quest build ships no x86 code, so supporting it would mandate
exactly the translation path this decision exists to avoid. Linux desktop only.

## Status, 2026-09-23

The x86_64/aarch64 split this document called for in `apk.rs`/`install.rs` was real
but incomplete: it stopped at the two constants and never reached the code that
actually *fetches* a build. `crates/cordial-update/src/provider/mirror.rs` hardcoded
`ABI_EXACT = "x86_64"` and searched archives for the literal path
`lib/x86_64/libroblox.so` rather than `apk::LIBRARY_IN_APK`; `provider/local.rs` and
`cordial-shell/src/install.rs` had the same hardcoding for the engine path, the split
APK filename, and Sober's own package directory. An aarch64 build would have compiled
clean and then found no build to run, silently, because the acquisition layer never
asked for or recognised anything but x86_64. Fixed 2026-09-23; see the commits
touching `provider/mirror.rs`, `provider/local.rs`, `cordial-shell/src/install.rs`,
`crates/cordial-update/src/deno.rs` and `justfile`.

One of those fixes was a genuine latent bug rather than a missing branch:
`cordial-shell/src/install.rs` rebuilt the split APK's filename from `HOST_ABI`
(`arm64-v8a`, hyphenated) instead of using `cordial_update::install::SPLIT_APK`
(`arm64_v8a`, underscored, which is how Play actually spells it). That is invisible on
x86_64, which has no hyphen to get wrong, and would have made this code path never find
the split archive on a real aarch64 install.

Packaging gates (`ExclusiveArch: x86_64` in what is now `packaging/rpm/stacked.spec`,
`Architecture: amd64` in `packaging/deb/control.in`, the AppImage's x86_64-linux-gnu
WebKitGTK discovery) were the same shape of gap and are fixed in the same batch of
commits, alongside aarch64 legs in `release.yml`/`flatpak.yml`/`test.yml`/`apt.yml`/
`yum.yml` — the pacman/AUR arch job stays x86_64-only, since Arch Linux ships no
aarch64 build at all (Arch Linux ARM is a separate project with its own repos).

**Corrected once more, 2026-09-24: the mirror fix above was itself wrong in a way
worth recording.** Making `ABI_EXACT` (`provider/mirror.rs`) equal to `apk::HOST_ABI`
made an aarch64 build ask APKPure for `x-abis: arm64-v8a` specifically — and for that
filter APKPure serves XAPK split bundles (`config.arm64_v8a.apk` nested inside a zip
this reader cannot look inside), not the monolithic all-ABI APK that `x-abis: x86_64`
returns and that this module's own header already measured. `ABI_EXACT` is now pinned
to the literal `"x86_64"` unconditionally, on both architectures — it selects
APKPure's bundle shape, not "the caller's own ABI" — with `apk::LIBRARY_IN_APK`
downstream still doing the real, correct, per-architecture check inside whatever comes
back. Known and documented gap: an ARM-only release newer than the current x86_64 one
(this already happened once, 2.735.1138, `docs/analysis/apk-mirrors.md`) would not
surface as "newest" on an aarch64 host. Real XAPK support in `held()`/`classify()`
would close that properly; not attempted, since it needs its own synthetic-XAPK tests.

**`pthread_mutex_t`/`pthread_attr_t` needed a real ABI shim, found by building, not by
inspection.** `crates/cordial-runtime/src/bionic/pthread.rs`'s size table was only
ever measured on x86_64, where bionic and glibc happen to agree on both types'
sizes. On aarch64 they do not (glibc's `pthread_mutex_t` is 48 bytes against bionic's
40; `pthread_attr_t` 64 against 56) — handing either straight to glibc, as the
pre-existing x86_64-only passthrough did, would have overrun the engine's own
allocation on the first mutex it locked. Both are now wrapped the same way `sem_t`
already was, aarch64-only; x86_64 is untouched.

**The page-size risk and ADR-001's re-verification are still open** — nothing here
closes either, and qemu-user emulation (4K-page x86_64 emulating 4K-page aarch64)
says nothing about real 16K-page hardware. What the "cheapest next step" paragraph
below asked for is now half-done rather than not-done: a real, Roblox-signed
arm64-v8a `libroblox.so` was fetched (via `cordial_update`'s own mirror path,
signature verified, fingerprint matches this module's own documented one) and loaded
under emulation — `cordial-run` got through the bionic linker, `JNI_OnLoad`, GameActivity
native init, and the engine's own flag initialisation (139 flags, by name) before the
smoke test's time bound ended, with no crash and no undefined symbol. The specific
`readelf --dyn-syms`/`DT_TEXTREL`/`p_align` check this paragraph asks for was not
run against that binary before it was deleted (per the disposable-profile instructions
for that test) — still open, and still cheap: needs one more fetch, not a device.

## Implementation notes

- Host ABI is fixed at **compile time**, not resolved at launch. `cordial_update::apk::
  HOST_ABI` is a `#[cfg(target_arch)]` constant, and `LIBRARY_IN_APK` and `SPLIT_APK` are
  spelled per-architecture beside it; an unsupported target fails with a `compile_error!`
  naming this document. Cordial never translates machine code, so the only library it can
  load is the one for its own architecture — which makes the ABI a property of the binary
  rather than something to detect. This paragraph previously described a run-time
  resolution that did not exist; the constants were two hardcoded literals in two crates.
- Watch the two spellings. The directory inside the APK is `lib/arm64-v8a/` with a hyphen;
  Play's split archive for the same ABI is `split_config.arm64_v8a.apk` with an underscore.
- Prefer the APK variant that carries the host ABI. If only a universal APK is available,
  load from the matching subdirectory and ignore the others. `apk::holds_with(apk, wanted)`
  already takes the path as a parameter and `engine_candidates` already loops over
  siblings, so this part needed no change.
- The engine cache is ABI-named — `~/.cache/cordial/lib/<abi>` — so two builds for two
  architectures against one home directory cannot overwrite each other.
- `libbadcpu` is gated on `CMAKE_SYSTEM_PROCESSOR` in `native/CMakeLists.txt`, and its
  `SIGILL` handler is installed only in the Roblox child process. It was **not** gated
  until 2026-08-26, and this line claimed a Meson `host_machine.cpu_family()` check in a
  project that uses CMake. `cpuid.cpp` includes `<cpuid.h>`, which does not exist on
  aarch64, so it was the first thing an ARM build would have hit. Nothing links the
  archive today, so gating it cost nothing.
- x86-64-v2 is the effective CPU floor. Below SSE4.1 the emulator cannot help enough and
  the correct behaviour is a clear hardware-too-old message, not a crash.
