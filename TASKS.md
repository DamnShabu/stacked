# Backlog from the mocktail audit

Source: `komaruworld/mocktail` @ 41 commits, Apache-2.0, cloned to
`../mocktail`. Apache-2.0 into GPL-3.0 is one-way compatible, so adoption is
permitted with attribution and a NOTICE entry. Everything below that involves
code is to be **written fresh against the problem**, not transcribed.

Cordial and mocktail vendor the same two load-bearing dependencies —
`ChristopherHX/libjnivm` and `mcpelauncher-linker` — so the comparison is real
rather than superficial.

---

## Corrections to the assumptions this audit started from

**mocktail uses SDL3, heavily.** The brief said "GTK4/libadwaita/Vulkan, no
SDL3". Its CMake builds `mocktail_platform_sdl`, `mocktail_sdl_vulkan_wsi` and
`mocktail_audio_sdl`, and links `SDL3::SDL3`. GTK4/libadwaita is the launcher
shell; SDL3 is the platform layer under the game surface. Cordial's canvas is a
Wayland subsurface of the libadwaita window with no intermediate layer
(ADR-011), which is a different and, for a Wayland-only target, simpler design.
**Recommendation: do not adopt SDL3.**

**Nine of the ten libraries proposed for adoption are not in mocktail.**
Checked against its `CMakeLists.txt` and `.gitmodules`:

| proposed | in mocktail |
|---|---|
| nlohmann/json | **yes** |
| capstone (not on the list, but present) | **yes** |
| mimalloc, detex, volk, fmt, mcl/oaknut, imgui, dyncall, libxml2 | **no** |

That list is Sober's dependency set as documented by `sober-oss`, not
mocktail's. Adopting it wholesale would be importing another project's
architecture, not this one's. Each is assessed separately below.

**Cordial has no CMake.** It is a Cargo workspace; the native side builds via
`build.rs` with Clang. So "extract `find_package`/`target_link_libraries`" has
no target here, and the `FetchContent` suggestion does not apply — the Cargo
equivalent for anything new is a normal crate dependency, or a git dependency
pinned by tag.

---

## Tier 1 — no code, testable immediately

### T1. `FStringGraphicsTextureManager2DenyPattern2 = ".*"` — DONE, unverified

**This is the "fix low resolution textures" commit** (`e161fec`), and it is one
line of configuration, not code.

`FStringGraphicsTextureManager2DenyPattern2` is **absent from Cordial's live
settings document**, so the engine uses its compiled-in default. mocktail sets
it to `".*"`, which denies every pattern and takes the engine off TextureManager2
entirely. The live set carries `FFlagTextureManager2SupportLegacyStreaming2 =
True`, so there is a legacy path to fall back to — consistent with this being
the intended escape hatch rather than a hack.

The related `FStringGraphicsTextureManager2DenyPattern` (no `2`) *is* in the
document, as `.*:.*:.*:1|.*:.*:.*:2|.*:.*:.*:3` — a tiered deny list. The
plausible reading is that unrecognised hardware falls into a low tier and gets
low-residency textures, and denying the manager outright restores full
resolution. **`INFERRED` — the causal story is not established, only the flag
value and its absence from our document.**

- **Touches:** Cordial's default flag layer only. No source change to the
  runtime or native side.
- **Status:** shipped in a new built-in flag layer (`flags.rs`), below plugins
  and the user so one line in `flags.json` overrules it. Confirmed applying, and
  confirmed overridable: `= USER-WINS from user (overrides built-in=.*)`.
- **Scope:** trivial. Isolated PR.
- **Verification:** still outstanding — requires a join and a visual comparison, since textures do
  not load on a startup-only run. Control by toggling the flag in the same
  session. The flag pipeline itself is proven — `FLogGraphics=0` takes
  `[FLog::Graphics]` from 30 lines to 0 on the same build.

### T2. `FStringGraphicsVulkanShaderMTDenyPattern` — append, do not replace

mocktail sets this to `"4318:.*"`. 4318 is `0x10DE`, the NVIDIA vendor ID, so
this disables Vulkan shader multithreading on all NVIDIA parts.

**Do not copy their value.** They *replace* the live string, which discards the
entries Roblox ships (`4112:*` and `1010:*` device pairs). Cordial should append
`|4318:.*` to whatever the document carries, so both the vendor's own deny list
and the NVIDIA workaround are in effect.

- **Touches:** flag layer, plus a small append-rather-overwrite helper if the
  current merge only supports replacement.
- **Scope:** trivial. Isolated PR.
- **Caveat:** unverifiable on this machine — the development box is an Intel
  13th-gen part, so the NVIDIA path cannot be exercised here. Ships as
  `INFERRED` unless someone with NVIDIA hardware confirms it.

---

## Tier 2 — real code, real gaps

### T3. ETC1 → BC1 texture transcoding (skybox and beyond)

mocktail's `dcf22a1` adds a 294-line ETC1→BC1 transcoder and wires it into the
asset path. The underlying problem is architectural and Cordial has it too:
**Roblox ships Android texture formats, and desktop GPUs do not implement
ETC1.** An engine that asks for an unsupported format gets a black or missing
surface, which is what "skybox issues" means.

This is the single most substantial thing in mocktail that Cordial lacks.

- **Touches:** new native module, plus the asset/AssetManager path in
  `native/`. Likely interacts with the graphics backend selection.
- **Scope:** major. Own PR, and **must be an independent implementation** —
  BCn and ETC block formats are specified publicly, so writing an encoder from
  the format specs is straightforward and avoids any question of derivation.
  Do not read their file while writing ours.
- **Prerequisite:** first establish that Cordial actually hits this. Look for
  ETC1/`VK_FORMAT_ETC2` requests or missing-format failures in a join run before
  building anything.
  **Partly established, 2026-09-29, on Intel (no NVIDIA hardware to hand):**
  the engine does query `vkGetPhysicalDeviceFormatProperties` for ASTC and BC
  directly at the signed-out landing screen (6 ASTC, 4 BC, 0 ETC2 queries in
  one run) — so this is a real code path, not merely a compiled-in assumption
  — and, with ASTC's answer masked to fully unsupported, TM2 selection and
  shader-module count were unaffected. That is evidence *against* this task's
  original framing ("shader translation for NVIDIA so TM2 works"), not for it.
  See [docs/analysis/nvidia-texture-manager2.md](docs/analysis/nvidia-texture-manager2.md)
  and [ADR-042](docs/adr/ADR-042-texture-format-query-observability.md). What
  remains open: in-game (as opposed to landing-screen) behaviour, and whether
  a real NVIDIA driver's `VK_FORMAT_ETC2_*` answers differ from what the mask
  assumes. No transcoder has been built; T3 stays unstarted pending either of
  those.

### T4. ANGLE fallback path

mocktail vendors `third_party/angle_headers` and gates behaviour on
`MOCKTAIL_DISABLE_AUTO_ANGLE_FALLBACK` and `MOCKTAIL_SOFTWARE_WINDOW_FALLBACK`.
Cordial has `CordialGraphicsBackend` deciding whether to offer the engine a
Vulkan loader, but no GL-over-Vulkan translation fallback.

Roblox renders through Vulkan now, so this is insurance for hardware where the
Vulkan path fails rather than a feature. **Lower priority than it looks.**

- **Touches:** `crates/cordial-runtime/src/graphics.rs`, backend selection.
- **Scope:** major. Deferred pending evidence that any target hardware needs it.

### T5. WebView

mocktail has a 1470-line `src/webview/` on WebKitGTK6. Cordial already links
WebKitGTK for the cookie/login path (`crates/cordial-runtime/src/cookies.rs`),
so the dependency is present and the gap is scope, not plumbing.

- **Touches:** `cordial-shell`, cookies module.
- **Scope:** moderate. Worth scoping against what it would actually be *for* —
  if it is the login flow, Cordial has that; if it is in-experience web content,
  that is a different and larger ask.

---

## Tier 3 — library assessments

Each judged on whether it solves a problem **Cordial has**, not on whether
another project uses it.

**mimalloc** (MIT) — general-purpose allocator, materially faster than glibc
malloc under heavy threaded allocation. Sober uses it; mocktail does not.
Cordial's engine is allocation-heavy, so this is a plausible general win. But
Cordial loads a bionic-linked engine through a glibc shim, and interposing an
allocator across that boundary is exactly where the shim's assumptions live.
**Effort: moderate, risk higher than the usual "just link mimalloc".** Wants a
measurement first: is allocation actually hot? Nothing here has measured that.

**detex** (ISC) — texture decompression for BCn/ETC/ASTC. Directly relevant to
T3, and the reason to consider it is precisely T3. If T3 proves real, detex is
the honest alternative to writing a transcoder: it is a small ISC-licensed C
library that already handles the formats. **Effort: trivial to link, moderate to
wire in. Assess together with T3, not separately.**

**nlohmann/json** (MIT) — header-only C++ JSON. mocktail uses it. Cordial's JSON
is on the Rust side with `serde_json`; the native side barely parses JSON at
all. **No need. Skip.**

**volk** (MIT) — Vulkan meta-loader, avoids the loader trampoline. A real but
small win, and only worth it if Vulkan call overhead shows up in a profile.
Cordial currently links `libvulkan.so.1` directly. **Effort: trivial. Value:
unmeasured, probably marginal. Skip until profiled.**

**fmt** (BSD) — C++ formatting. Cordial's native side uses `fprintf`/`snprintf`
throughout and the codebase has a consistent voice built around that. Adding fmt
would mean either a mixed style or a sweep. **Skip.**

**mcl / oaknut** (MIT, merryhime) — ARM64 code emission. These exist to
*generate* AArch64 instructions, which is a JIT/binary-translation concern.
Cordial loads the **x86-64** Android build natively on x86-64 hardware; there is
nothing to translate. **This is Sober-architecture-specific. Do not adopt.**

**dyncall** (ISC) — dynamic FFI calls with runtime-constructed signatures. Same
category: it exists for a translation layer that must call across an ABI it
learns at runtime. Cordial's JNI boundary is statically typed through libjnivm.
**Sober-specific. Do not adopt.**

**libbadcpu** — already vendored in Cordial's `third_party/`. Listed in the
brief as something Cordial lacks; it does not.

**imgui** (MIT) — immediate-mode debug UI. Cordial's UI is libadwaita and its
diagnostics are logs and traces. An imgui overlay would be a genuinely useful
*debug* surface, but it is a new UI stack for a project that deliberately has
one. **Skip unless a specific debugging need appears that logs cannot serve.**

**libxml2** (MIT) — XML parsing. Nothing in Cordial parses XML. **Skip.**

**capstone** — in mocktail, not on the proposed list. Disassembly framework;
mocktail presumably uses it for diagnostics. Given AGENTS.md's rule that reading
the stripped binary has been wrong nine times running and running it has never
been, **adding a disassembler is pointed the wrong way for this project. Skip.**

**AOSP portions** (Apache-2.0) — already present as the ported bionic linker.

**SDL3** — see the correction above. **Do not adopt.** If gamepad support is
wanted, `libmanette` is the GTK4-native option and does not bring a second
platform layer.

**libplacebo** — not used by mocktail. It is a GPU video/image processing
library aimed at scaling, tone-mapping and colour management. Cordial presents
the engine's own Vulkan output; there is no processing stage for it to live in.
**Skip unless a post-processing pipeline is on the roadmap.**

---

## Order of work

1. **T1** — one flag, plausibly fixes a visible quality problem, trivial to
   revert. Do this first.
2. **T3 prerequisite** — establish whether Cordial hits the ETC1 gap at all.
   One join run with format logging answers it and costs nothing.
3. **T2** — trivial, but unverifiable on current hardware; ship as `INFERRED`.
4. **T3 proper** — only if step 2 confirms it, and with detex assessed as the
   alternative to hand-writing a transcoder.
5. **T5** — scope the actual requirement before writing anything.
6. **T4** — deferred.

Nothing above has been implemented. `INFERRED` markers are load-bearing: T1's
mechanism, T2 entirely, and T3's applicability to Cordial are all unestablished.

---

## Direct verdicts on the proposed libraries

| library | verdict | why |
|---|---|---|
| **mimalloc** | **yes — adopt** | See the note below. The earlier "measure first" verdict was over-cautious. |
| **detex** | **yes, if T3 is real** | It is the honest alternative to hand-writing an ETC1→BC1 transcoder. Tied to T3; assess together. |
| **nlohmann/json** | **no** | Cordial's JSON is `serde_json` on the Rust side. The native side barely parses JSON. |
| **volk** | **no** | Saves the Vulkan loader trampoline. Unmeasured, probably marginal. Revisit only if a profile shows it. |
| **fmt** | **no** | The native side is `fprintf`/`snprintf` throughout with a consistent voice. Adding fmt means a mixed style or a sweep, for no capability. |
| **mcl / oaknut** | **no** | AArch64 instruction emission. Cordial loads the **x86-64** build natively on x86-64. Nothing to emit. Sober-architecture-specific. |
| **dyncall** | **no** | Runtime-signature FFI, for a translation layer that learns ABIs at runtime. Cordial's JNI boundary is statically typed through libjnivm. Sober-specific. |
| **imgui** | **no** | New UI stack for a project that deliberately has one (libadwaita). Diagnostics here are logs and traces. |
| **libxml2** | **no** | Nothing in Cordial parses XML. |
| **AOSP portions** | **already have it** | The ported bionic linker is exactly this. |
| **SDL3** | **no** | mocktail uses it as its platform layer under a GTK shell. Cordial's canvas is a Wayland subsurface of the libadwaita window (ADR-011) and works. Two platform layers means two event loops competing to be the Wayland client. |
| **libplacebo** | **no** | GPU scaling/tone-mapping/colour. Cordial presents the engine's own Vulkan output; there is no processing stage for it to live in. |

## Input, voice, camera, VR — what the engine we load actually supports

Measured by class-name presence across the three dex files of the shipped
x86-64 Android build, `2.730.0.790`:

| surface | hits in the dex | verdict |
|---|---|---|
| `hardware/camera2`, `CameraDevice`, `CameraManager` | **89** | reachable |
| `WebRtcAudio` | **40** | reachable, and partly hooked already |
| `InputDevice`, `MotionEvent` | **21** | reachable |
| `AppRtcDevice` | **5** | reachable — this is the real voice path |
| `Oculus`, `OpenXR`, `Cardboard`, `GvrLayout` | **0** | **not present** |

### VR is not available and this is not a Cordial limitation

**The reasoning above is incomplete, though the verdict holds.** A dex
class-count of zero only shows Roblox's own Java code has no VR surface — it
says nothing about what the engine asks the *platform* for, and Cordial
supplies that platform, not Roblox's dex (see issue #35, `DeviceUtils`, for a
case where the engine asked for a class no dex declared). The binary does have
VR code compiled in: flag-shaped strings (`IsVRAppBuild`, `ExposeOpenXrAPI1`,
`DebugEnableVREmulator` and more) and `[FLog::VRService]` format strings for
per-eye camera CFrames are present in `libroblox.so`.

What actually settles it: [docs/analysis/vr-reachability.md](docs/analysis/vr-reachability.md)
turned on every VR-related FastFlag findable in `docs/traces/` (and the
binary-string names besides, under both `F`/`DF` prefixes), verified the
override reached the engine (`nativeInitClientSettings -> 0`, settings
document grew by exactly the injected bytes), and compared against an
otherwise-identical control run. Across three instruments — the dumped Java
class surface, the `Constructed Unresolved symbol` / stub-call log, and the
engine's own log stream with `FLogVRService`/`DFLogVRService` turned on — the
two runs are indistinguishable. No new class or method is requested, no
unresolved symbol appears, no VRService line is ever printed, and a
`CORDIAL_TRACE_PATHS=1` capture shows no attempt to `dlopen` a VR runtime
library. The flags do nothing observable, which is a different and stronger
finding than "no VR classes in the dex" — reached at the account-router/menu
shell, not inside a joined place, which that document names as the one gap
still open.

**Reopened 2026-09-16, and the verdict above — "Closed, not deferred" — was
wrong.** It closed on a run that the same sentence admits never reached a
joined place, which is where VR initialisation would happen. A second gap went
unrecorded and matters more: the machine carried no OpenXR runtime at all. No
loader, no `/usr/share/openxr/1/`, `XR_RUNTIME_JSON` unset, verified on this
host. `ExposeOpenXrAPI1` and `DebugEnableVREmulator` cannot produce an
observable effect, in a joined place or anywhere else, when there is nothing
for the engine to find, and "no `dlopen` of a VR runtime library" is what a
machine with no VR runtime installed looks like either way. Three instruments
agreed because none of them could distinguish the two cases -- the same shape
as every other broken measurement this file records.

Settling it from the other direction: a working implementation exists against
this same x86-64 APK and this same engine, running the OpenXR integration
host-side, outside `libroblox.so`, at roughly 15,500 lines. So the engine's VR
path is reachable, and "zero OpenXR symbols imported by `libroblox.so`" never
ruled it out. On this architecture Cordial supplies the platform, so a runtime
integration lives in the host by construction and no imported symbol would ever
appear in the engine. The symbol scan answered a question nobody needed asked.

**That arm has since been run, and it went against the hypothesis.** Three
runs to the Landing page, each with its own data root, repeated: flags off;
140 VR flag overrides on with no OpenXR runtime; and the same 140 with Monado
25.1.0 serving and `XR_RUNTIME_JSON` set. The dumped Java class surface is
byte-identical across all three at 4584 lines, no `Constructed Unresolved
symbol` appears in any arm, the stub tally is unchanged, and the normalised
stderr streams of the two flag-on arms diff to zero lines -- reproduced across
two independent runs of the Monado arm. A discoverable OpenXR runtime changes
nothing, so the gap this section opened is now closed by measurement rather
than by assumption.

The platform-layer route is closed too, for a more basic reason than the
experiment: **there is no VR-shaped question for Cordial to answer.** The one
VR item anywhere in the observed JNI surface is
`NativeGLJavaInterface.onVrSessionStateUpdate(I)V`, and it is outbound -- the
engine notifying Java that a session exists, not a capability the engine
consults. It appears in the 2026-08-06 `CORDIAL_JNI_TRACE` capture (the one
that reached a joined place) only as a `jmethodID` cached during
`JNI_OnLoad`'s blanket pass, never invoked. `PackageManager.hasSystemFeature`
was deliberately never implemented here because nothing has ever been seen
calling it, so there is no gate sitting unanswered. Issue #35's `DeviceUtils`
precedent is real but does not transfer: supplying a class the engine asks for
is not the same as inventing one it never asks about.

What remains genuinely untested is a signed-in, joined place -- whether flags
plus a runtime make `onVrSessionStateUpdate` actually fire inside an
experience. That needs an account, which is a user decision, not an agent's.
The only demonstrated way to construct a VR device is a raw byte written at a
hardcoded per-build address, which [ADR-001](docs/adr/ADR-001-in-process-hooking.md)
and [ADR-003](docs/adr/ADR-003-plugin-isolation.md) make absent rather than
disabled, and which would break on every Roblox update besides. **Not closed,
and not pursued: blocked on a route this project does not take.**

### Voice chat — implemented, broader testing remains

Dual protocol/async permission delivery now unlocks voice initialisation, and
AAudio supports the callback-driven microphone input Roblox requests. A tester
confirmed audible transmission on Roblox 2.738.0.1397 in the combined local
build. Other devices and distributions remain unverified. See
[the controlled experiment](docs/analysis/voice-dual-response.md).

<details>
<summary>Superseded investigation notes, before the 2026-09-14 fix</summary>

The following records earlier observations and hypotheses, not the current
blockers or next steps.

`AppRtcDeviceWrapper` used to be registered with `isValid()` answering
**false** and nothing else. It now answers true and implements the device
getters, start/stop communication and the call mute
(`native/unanswered_classes.cpp`), with the mute zeroing samples in
`AAudioStream_read`. **The engine has never called any of it**: every method
logs as `I/Cordial-Voice`, and three runs in a voice place printed none. It is
kept because the contract comes from the dex and the Android capture, not
because it fixed anything.

Voice does not join under Cordial. In place 14236925335 with
`FLogVoiceChatLogs=7`, under `roblox-app`, `android-tablet` and the default
`pc-windows-11` (runs on 2026-09-13 logged the constructor under all three), the engine
logs `voice_service_init`, `IsVoiceEnabledForUserIdAsync ... true`,
`VoiceChatInternal ctor`, a muted publish pause and
`onServiceProvider - UseNewJoinFlow = 0`, then no further voice line.

**The Sober comparison this paragraph rested on measured nothing.** Sober on
the same account, place and build logged no `VoiceChatLogs` line at all while
voice joined and toggled, and that was read as "a working join logs the same
silence". A second Sober run on 2026-09-14 shows why: with both families in
its `fflags`, it wrote `DFLog*` lines (`VoiceChatRequestTrace`,
`HttpTraceError`) and no `FLog*` voice line, so the `FLog` channels were never
on in Sober. Whether a working join logs `ClientJoinOperation Constructed`,
`asyncInitRTCThreadsAndAudioDevice` or `ClientPublishOperation` is untested;
none of the three has appeared in any Cordial run. What the comparison does
show is where the two split: Sober was already in voice when the game loaded,
since the mic button it toggled only exists after a join, while Cordial's top
bar still offers the join-voice button. The fault is the automatic join at
load, not the button. The only line the toggles
produced in Sober was a re-enumeration of the audio devices,
`[FLog::Audio] InputDevice 0: ... 48000/2/4`, once per toggle. Cordial's
Neighbors runs log that line only before the join, never after it, and report
`48000/1/4`: Cordial's AAudio input answers mono (`kCaptureChannels` in
`native/aaudio.cpp`) where Sober answers stereo. Whether the channel count
matters is untested. `DFLogHttpTraceLight`, the channel that prints URLs, stops
0.76 s after launch when Roblox's settings fetch replaces DF overrides;
`DFLogHttpTrace` carries on for the whole run but prints no URLs, so in-game
voice HTTP is still not visible.

The microphone permission ask comes before any of that: the engine sends it
over the message bus on `PermissionsProtocol`, not through
`checkSelfPermission`, and nothing answered it. `crates/cordial-runtime/src/permissions.rs`
now binds it and grants `MICROPHONE_ACCESS` alone. In Neighbors a voice click
sends `PermissionsRequest`, the `AUTHORIZED` answer is delivered inline with
(id, response) order and no `PermissionsProtocolCore: Invalid response
received.`, and then nothing follows: no AAudio input stream, no `dlsym`, no
voice log line, no UDP socket. The reverse order ends the run with
`bad_function_call`, and answering from another thread after `run()` returns
kills it, so both of those are measured rather than inferred.

The bursts of `status:429` on `voice.roblox.com/v1/settings` (33 at once) and
`/v1/user-voice-connection-state` (43 in ten seconds) came from runs with
repeated clicking; a single click in a clean run produced neither, so they
look like a consequence rather than the cause. The one route left to the
voice settings responses is intercepting the engine's own HTTPS: curl reads
`HTTPS_PROXY` through the host `getenv` (`crates/cordial-shell/src/network.rs`)
and trusts the extracted `assets/ssl/cacert.pem`, so a local proxy whose CA is
appended to that file should see them. That has not been tried.

</details>

### Gamepad and extended input

**Partly done, and blocked on one integer.** `android/gamepad.rs` and the six
`NativeInputInterface` trampolines in `native/game_activity.cpp` are in place;
the feature is off unless `CORDIAL_GAMEPAD=1`, because the engine cannot be told
a pad exists without being told what *kind* of pad it is, and nothing available
here says what the values mean.

**Two things this entry used to say were wrong, and are corrected here rather
than deleted, because both are easy to re-derive.**

It recommended `libmanette`. There is no Rust binding: `cargo search libmanette`
returns nothing, and `cargo search manette` returns one unrelated crate. Adopting
it means hand-written GObject FFI, so the recommendation was not actionable as
written. **gilrs** is the real candidate — it carries `SDL_GameControllerDB` and
exposes the vendor and product ids a type classifier wants — but it pulls
`libudev-sys`, whose `build.rs` is an unconditional
`pkg_config::find_library("libudev").unwrap()` and whose output links
`libudev.so.1`. Cordial has no udev in `Cargo.lock` today, so that is a
packaging decision (flatpak, deb, rpm, AppImage, CI) and not an implementation
detail. Until it is taken, `gamepad.rs` reads `/dev/input/js*` directly with no
new dependency and a fixed mapping table for the standard layout.

It also pointed the work at the GameActivity motion-event path. That is the pipe
`native/game_activity.cpp` records as accepted-and-ignored — every click
delivered through `onTouchEventNative` was taken by the engine and nothing on
screen moved, pixel-identical before and after. Gamepad goes through
`NativeInputInterface`, for the same reason the mouse does.

**The blocker.** `nativeGamepadConnectEventWithGamepadType(II)V` is the only
connect entry point this build has — the type-less `nativeGamepadConnectEvent`
returns `no match` from `tools/dex_method.py` and `0` from `readelf | grep -c`.
`docs/traces/waydroid-roblox-startup.log.gz` has no gamepad line in it, because
the capture was taken with no pad attached; the dex declares no gamepad class;
mocktail resolves all six symbols and never calls one. Either of these settles
it:

1. **Re-capture the logcat with a controller attached.** This is the project's
   own "grep the traces first" rule, and it has never been run for this question.
   Needs a pad and the Waydroid setup in `docs/traces/README.md`.
2. **Sweep the argument against the engine's own glyphs.** The binary carries
   three glyph folders — `DefaultController`, `PlayStationController`,
   `XboxController` — so the UI is a readout of the type. `CORDIAL_GAMEPAD=1
   CORDIAL_GAMEPAD_PROBE=1 CORDIAL_GAMEPAD_TYPE=N` announces a synthetic pad with
   no hardware at all; read the frame with `cordial_screenshot` and sweep N. A
   different N drawing different glyphs is the control.

Getting this wrong is not theoretical — Sober #1018 is *"Sober detects my PS4
controller as a XBOX one"*.

- **Touches:** `crates/cordial-runtime/src/android/gamepad.rs`,
  `android/input.rs`, `native/game_activity.cpp`, `cordial-linker-sys`.
- **Scope:** the remaining work is one measurement, then a mapping layer.

### Camera

89 hits on Camera2. The engine expects an Android `CameraManager`/`CameraDevice`
and Cordial answers none of it, so anything camera-driven silently does nothing —
the `broken_feature` shape from AGENTS.md.

- **Touches:** new `native/camera_classes.cpp`, a PipeWire/v4l2 source.
- **Scope:** major. Lower priority than voice unless something specific needs it.

**Order for these four:** gamepad (moderate, isolated, immediately useful) →
voice (major, best understood) → camera (major, no current demand) → VR (closed).

---

## mimalloc — revised to yes

The earlier verdict here was "try it, measure it", which was over-cautious. The
case for adopting is stronger than that, on three grounds that do not require a
profile first:

**The workload is mimalloc's designed-for case.** A game engine doing many small,
short-lived allocations across many threads is precisely the pattern mimalloc's
free-list sharding and thread-local heaps target, and precisely where glibc's
malloc arena contention costs most. This is not a general "allocators are faster"
claim; it is a specific match between this workload and this allocator.

**Sober uses it, and Sober works.** When the reference implementation that reaches
gameplay has made a choice and we are diverging from it for no reason, the
divergence is the thing that needs justifying, not the adoption.

**It is cheap and trivially revertible.** Link mimalloc's override into
`cordial-run` so it wins symbol resolution ahead of glibc; remove one line to
undo it.

**The one real technical question, which is not a hedge.** Cordial runs with
`--host-libc`, so the engine resolves libc symbols through the host, and the AOSP
bionic linker does its own symbol resolution. Whether mimalloc's override
actually captures the *engine's* allocations — as opposed to only Cordial's own —
depends on how that resolution binds `malloc`. If the ported linker binds it from
`libc.so`'s handle directly rather than through the global symbol table, the
override captures nothing and the change is a no-op wearing a performance badge.

That is checkable rather than arguable: mimalloc reports allocation statistics
(`MIMALLOC_SHOW_STATS=1`). If the engine's allocations are flowing through it,
the counts will be large; if they are not, they will be Cordial-sized. **Do that
check first — not to decide whether to adopt, but to know whether the adoption
did anything.** Shipping a no-op as a performance improvement is the exact
failure mode AGENTS.md exists to prevent.

- **Touches:** `crates/cordial-runtime` dependency list, one `#[global_allocator]`
  or link-order change in `cordial-run`. Possibly the linker's symbol table
  handling if the check comes back negative.
- **Scope:** trivial to add, moderate if the shim needs to route it.
- **Priority:** high. Do it after T1.

---

## Client integrity: mocktail does not address it

Asked directly and answered plainly, because the possibility was worth checking
and the answer is no.

Across all 41 commits there is **no** commit mentioning integrity, signature
verification, anti-cheat, Hyperion, Byfron, bans, or disconnects. Searching the
source, the only `integrity` hits are `src/update/payload_integrity.cc`,
`candidate_approval.cc` and `apk_bundle.cc` — that is *update payload* integrity,
verifying a downloaded APK bundle before installing it. Unrelated to client
attestation. There is no handling of a 304 or any server-initiated disconnect
anywhere in the tree.

So mocktail is not a source of a fix for the error Cordial hits at 60 seconds.
It is a useful source for texture handling, platform layering and packaging, and
it is silent on this. Anyone reading this backlog hoping the comparison would
close the 304 should stop here rather than go looking.

---

## mimalloc: measured, and the answer is no. Roblox already ships it

Adopting was agreed on the argument that Roblox's workload — many small
allocations, many threads, CPU-bound — is mimalloc's designed-for case. The
argument is sound. It does not apply, because the engine got there first.

`libroblox.so` imports **no allocation entry point at all**. Filtering its 578
undefined symbols for `malloc`, `calloc`, `realloc`, `free`, `reallocarray`,
`posix_memalign`, `aligned_alloc`, `memalign`, `malloc_usable_size`, `strdup`,
`strndup` and the whole `operator new`/`delete` family leaves **`realpath` and
`vasprintf`** and nothing else. The C++ allocation operators come back **0**.

An engine that never calls the system allocator cannot be given a different one.
And `strings libroblox.so` says why:

    10  mimalloc
    48  Mimalloc

**Roblox links mimalloc statically into the engine.** So it is already running
under exactly the allocator this task proposed to give it, and the version it
uses is Roblox's own choice rather than ours.

Overriding `malloc` in `bionic::function_overrides` would therefore have reached
only Cordial's own Rust allocations, which are trivial next to the engine's. That
is the no-op-wearing-a-performance-badge outcome, and it is now measured rather
than guessed at.

**Do not adopt.** This also weakens the "Sober does it, so should we" reasoning
generally: Sober's mimalloc serves Sober's own process, not the engine's heap.

## detex: premise unproven, do not vendor yet

The engine already knows the formats. `strings libroblox.so`:

    DXT 21   ASTC 14   KTX2 14   ETC2 10   ETC1 7   BC3 3   BC7 1   BC1 0

and it links no third-party decoder — no detex, bcdec, etcdec, rgbcx, astcenc.
So format handling is the engine's own, and whether it ever hands the driver
something a desktop GPU cannot take is a **runtime** question that no amount of
reading answers.

Vendoring detex before knowing that is the same mistake mimalloc nearly was.
**The measurement first:** one join with the graphics log turned up, recording
which `VK_FORMAT_*` the engine requests and whether any are refused. If ETC
formats reach the driver, detex earns its place; if the engine transcodes to DXT
itself, it does not.

**A first pass at this measurement landed 2026-09-29** (Intel only, no NVIDIA
to hand): the engine does reach the driver for ASTC and BC format properties at
the landing screen, on every request seen the driver answered "supported" —
Intel's Xe-LP hardware natively decodes both. ETC2 was never queried in that
window. See [docs/analysis/nvidia-texture-manager2.md](docs/analysis/nvidia-texture-manager2.md).
detex's place in this decision is unchanged: nothing has yet shown a format the
driver refuses, on any hardware this project has access to.

## JNI surface diff — 24 classes mocktail names that Cordial does not

The comparison that is actually apples-to-apples between a Rust project and a C++
one: not source, but which Java classes each answers. Cordial answers 62;
mocktail names 86.

Spot-checked rather than trusted, four of the five confirmed absent from Cordial:

| class | Cordial | mocktail |
|---|---|---|
| `com/roblox/engine/jni/memstorage/MemStorage` (+ `Connection`, `Callback`) | **absent** | yes |
| `com/roblox/engine/jni/autovalue/StartGameParams` | **absent** | yes |
| `com/roblox/engine/jni/EngineJavaCallback2` | **absent** | yes |
| `android/hardware/SensorManager` | **absent** | yes |
| `com/roblox/engine/jni/NativeAppBridgeInterface` | present | yes |

Also named by mocktail and unanswered here: `RobloxActivity`,
`MainGameActivity`, the `localstorageplatforminterface/generated/` family,
`PackageManager`/`PackageInfo`/`ApplicationInfo`, `DisplayManager`, `Display`,
`InputMethodManager`, `SurfaceView`/`SurfaceHolder`/`View`/`Window`/
`WindowManager`, `ViewRootImpl`, `Context`.

**`MemStorage` is the most interesting.** The engine exports
`MemStorage_bind`, `_fire`, `_getItem`, `_setItem`, `hasItem`, `removeItem` and
`Connection_disconnect`/`_releaseConnection` — a complete key-value channel with
a subscription mechanism — and Cordial answers none of it. **`StartGameParams`**
is second: it is on the join path, which is where Cordial's remaining problem
lives.

Two honest caveats. mocktail *naming* a class is not proof it implements it
usefully. And this diff was built by grepping `GetClass("...")` out of Cordial
and string literals out of mocktail, so a class Cordial answers by another route
would read as absent. **This is a lead list, not a verdict** — each entry needs
confirming against a JNI trace before anyone builds to it.

**Next:** run one `CORDIAL_JNI_TRACE=1` join and intersect the unresolved-symbol
lines with this list. That turns 24 leads into the subset the engine actually
asks for, and it costs one run.

---

## Correction: the 24-class list, re-read against the dex

The "Cordial answers none of `MemStorage`" framing was wrong in a way that
changes what the work is.

`MemStorage`'s methods are **engine natives**. `libroblox.so` exports
`Java_com_roblox_engine_jni_memstorage_MemStorage_bind`, `_fire`, `_getItem`,
`_setItem`, `hasItem`, `removeItem` and `Connection_disconnect`/
`_releaseConnection`. So the engine *implements* that channel and the application
*calls* it. There is nothing for Cordial to answer there, and an implementation
would be shadowing the engine's own.

What Cordial does owe is the two classes the engine constructs and calls back
into, exactly the `NativeFlagsInitResult` pattern:

    com/roblox/engine/jni/memstorage/Connection   <init> (J)V
                                                  disconnect ()V
                                                  finalize ()V
    com/roblox/engine/jni/memstorage/Callback     (engine calls into it)

`Connection` takes a native handle as a `jlong`, so it is a real, well-defined
object with no guessing involved. **Safe to implement.**

### `EngineJavaCallback2` is not safe to implement blind

Its entire surface is obfuscated — `a(I)V`, `b(J)V`, `c(I)V`, `d()V`,
`e(String)V`, through to `q(JZ[BLcom/roblox/engine/jni/model/NativeTextBoxInfo;)V`
— seventeen single-letter methods whose meanings the dex does not carry.

They are all `void`, so *receiving* them is honest in the way
`NativeHelper`'s lifecycle callbacks are: an announcement's honest answer is to
have received it. But anything beyond logging the call would be inventing
semantics. **Implement as receivers that log, and nothing more, and say so in
the comment.** Do not let a later reader mistake a named parameter for known
meaning.

### And the startup trace does not settle this either way

A `CORDIAL_JNI_TRACE=1` startup on 2.734.0.917 leaves only five classes
unresolved — `NativeGLJavaInterface` (14), `GameActivity` (6), `java/util/List`
(2), `java/lang/Class` (2), `NetworkUtils` (2). None of the 24 appear.

That is **not** evidence they are unneeded. `KeyRing` was recorded earlier in
this same investigation as reaching nothing on startup and logging normally on a
join, and `StartGameParams` is by its name a join-path class. A ten-second
startup trace only proves what startup asks for. The list stands; the trace to
settle it has to be a join.

## Discord Rich Presence: it exists and it works

Asked as "where is the plugin". It is at `plugins/discord-presence/` —
`plugin.json` declaring `lifecycle.read`, `presence.set` and `log`, plus
`main.ts`. The host side is `crates/cordial-plugins/src/presence.rs`, ADR-007's
worked example.

It is not a sketch. Four tests pass, including an end-to-end one that runs the
real shipped plugin:

    presence::tests::presence_set_speaks_discords_framing_to_a_local_socket ... ok
    presence::tests::details_over_the_discord_limit_is_refused ... ok
    presence::tests::a_payload_rejects_fields_discord_does_not_define ... ok
    discord_presence_follows_lifecycle_events_all_the_way_to_the_wire ... ok

**The gap is documentation, not implementation.** `README.md` mentions plugins in
general and links the Discord server, and never mentions that a Discord presence
plugin ships or how to enable it. `site/index.html` is the website. Both want a
section. That is a writing task, not an engineering one, and it should not be
filed as "fully implement Discord RPC" — the misfiling is the bug.

---

## WebView: the protocol is engine-side and self-describing

The highest-value thing the mocktail comparison produced. mocktail answers
`com/roblox/protocols/webview/WebViewProtocol`; **Cordial answers none of it**,
which is why account settings, Robux purchase and anything else that opens a web
window do nothing.

`libroblox.so` exports **23** natives for it, and the shape they describe is a
complete protocol the application only has to provide a window for:

    initializeAndroidWebViewProtocol      the app registers itself as the provider
    getProtocolName                       the channel's name
    getOpenWindowId / getCloseWindowId    message ids the engine will send
    getMutateWindowId / getHandleWindowCloseId
    getIsAvailableId
    getUrlKey / getTitleKey / getWindowTypeKey / getSearchParamsKey
    getSearchTypeKey / getIsVisibleKey / getHideHeaderKey
    getBackButtonVisibleKey / getShowDomainAsTitleKey / getAvailableKey
    getFFlagWebViewHasBackButtonVisible / getFFlagWebViewHasHideHeader
    signalJavascriptCallback              the app returns a JS result to the engine

Plus `DomainAllowListChecker` — `checkDomainAllowList`,
`isKnownTrustedDomain`, `enableDomainAllowListChecker` — also engine-side.

**Nothing here needs guessing, and that is the point.** Every message id and
every payload key is obtained by *calling the engine's own getter*. An
implementation that hardcodes `"url"` because that is what the key is probably
called would break silently on the next build; one that calls `getUrlKey()` cannot.
Write it that way.

### Design notes worth taking, in our own terms

mocktail runs its webview **out of process** — `mocktail_webview_helper.cc` has
its own `main()` and is launched by a separate launcher — with a JS bridge named
`executeRoblox` and an origin check on every message
(`rejected executeRoblox from untrusted origin`).

Both properties are right and both match decisions Cordial has already made.
Out-of-process isolation is ADR-003's reasoning applied to a browser engine: a
WebKit crash or a hostile page cannot reach the Roblox process. And origin
validation is not optional — this window is where the user signs in and where
payment happens, so a page that can post arbitrary commands to the engine is the
whole security boundary.

Cordial already links WebKitGTK for the cookie path, so the dependency is
present. What is missing is the protocol side and the window.

- **Touches:** new `native/webview.cpp` for the protocol surface, a new
  out-of-process helper binary, the cookie module for the shared session.
- **Scope:** major, and the largest single feature gap Cordial has.
- **Isolated?** Mostly. The cookie/session sharing is the one piece that touches
  existing state.
- **Do not transcribe.** mocktail's helper is 45 KB of WebKitGTK plumbing under
  Apache-2.0. The protocol above is read off the engine's own exports and the
  dex, which is ours to implement freshly. Write the helper from the WebKitGTK
  documentation, not from their file.

### Still not established: whether mocktail survives past 60 seconds

`space.bigrat.mocktail` 1.0.3 is installed. Its 41 commits and its source contain
no integrity, anti-cheat or 304 handling — but that is consistent both with it
never hitting the error and with it hitting the error and nobody having written
anything down. **It has not been run.** Until it is, "mocktail does not get the
integrity error" is an assumption, and this document should not be read as
supporting it.

---

## Input: Cordial answers 7 of the engine's 22 entry points

Counted against `libroblox.so`'s exports, not guessed.

**Cordial calls:** `nativePassKeyEvent`, `nativePassText`, `nativePassMouse`,
`nativePassMouseButton`, `nativePassMouseMove`, `nativePassMousePan`,
`nativePassMouseWheel`.

**The engine also exports, and nothing here calls:**

    nativePassInput                   nativePassInputBatch
    nativePassTapGesture              nativePassSwipeGesture
    nativePassPinchGesture            nativePassRotateGesture
    nativePassLongPressGesture        nativePassMousePinch
    nativePassPanGestureWithVelocity  nativePassPanGestureMultitouch
    nativePassTouchEndVelocity
    nativePassAccelerometerChange     nativePassGravityChange
    nativePassGyroscopeChange
    nativePassCurrentDisplayRefreshRate  nativePassSupportedRefreshRates

### The two worth doing first are not the gestures

`nativePassCurrentDisplayRefreshRate` and `nativePassSupportedRefreshRates` tell
the engine what the display can do. AGENTS.md records that with input flowing the
frame rate is a hard FIFO vsync lock to the output's refresh — 60 Hz gives 60, a
50 Hz monitor gives 49.4 even in fullscreen at four times the pixels. **Cordial
has never told the engine what refresh rates exist.** Whether that is why the
lock is so rigid is untested, but it is the one place a client can say something
about refresh and Cordial says nothing.

- **Touches:** the input path in `crates/cordial-runtime/src/android/`, plus
  whatever reads the Wayland output mode.
- **Scope:** small. Isolated PR. Measure with input flowing, per AGENTS.md, and
  report the input rate beside the frame rate.

### On IME and text: there is nothing to copy — corrected

The question was how mocktail types. The answer is that **it does not call
`nativePassText` at all** — the name does not appear in its input surface.
Instead it carries `roblox_text_editor.cc` (788 lines),
`roblox_text_input_jni_bridge.cc` (799), `roblox_text_surface_overlay.cc` (598),
`roblox_text_display_state.cc` (305) and two input routers — **3840 lines**
reimplementing text editing above the engine.

Cordial drives `syncTextboxTextAndCursorPosition2` per keystroke, which fills
whichever box has focus, and keeps `nativePassText` for the finish — on Android
that is the soft keyboard delivering final text and dismissing itself. Driving
both per character was tried and produced "type one letter, box blurs", which is
in `input.rs`'s own comment with the trace that showed it.

**Corrected after this was first written: mocktail does not show text as you type
either.** So its 3840 lines are not a working implementation Cordial is missing,
they are a different unfinished attempt at the same problem. There is nothing to
borrow, and text entry is a shared gap rather than a comparative one — the same
shape as `AppRtcDeviceWrapper` and voice.

Which means whatever is wrong with typing in a Roblox text box has to be found
here, from a trace, rather than read off their tree. `CORDIAL_TRACE_TEXT=1` prints
the focus handle and the text sync per keystroke, and the three failures it
distinguishes — the box never focuses, it focuses and blurs immediately, or it
holds focus and the characters do not arrive — want different fixes.

What is worth taking from that area is the gesture and sensor list above, which
is a list of calls rather than a design.

---

## Audio, compared against mocktail: nothing to do

Read-only comparison of `native/opensles.cpp`, `pipewire_backend.cpp` and
`audio_classes.cpp` against mocktail's `src/audio/`. **No bugs found, and nothing
missing relative to theirs.** Recorded so the next person does not repeat it.

**Cordial is ahead here, in two ways worth protecting from a later "cleanup":**

*The OpenSL recorder is real.* `native/opensles.cpp` implements the full
`SLRecordItf` over PipeWire, opening and closing a capture stream on the actual
`RECORDING`/`PAUSED`/`STOPPED`/`Destroy` transitions. mocktail's
`EngineCreateAudioRecorder` unconditionally returns
`kResultFeatureUnsupported` — recording through OpenSL is simply refused there.

*The Android audio classes are answered.* Cordial implements
`android/media/AudioManager`, `AudioRecord`, `AudioDeviceInfo`,
`WebRtcAudioManager` and `WebRtcAudioRecord` with real PipeWire-backed device
data. mocktail registers `AudioManager` with **zero method hooks** and has none of
the others. A build that calls `AudioManager.getDevices()` gets a real answer from
Cordial and jnivm's untyped fallback from mocktail.

### A correction

`AppRtcDeviceWrapper` has been described in this backlog as the real voice path
that Cordial implements none of, with the implication that mocktail does.
**It does not.** `grep -r AppRtcDeviceWrapper` returns nothing in mocktail's tree.
The class is real — `libroblox.so` exports
`Java_com_roblox_audio_AppRtcDeviceWrapper_nativeAudioDeviceChanged` — but it is
unimplemented in *both* projects. Voice chat is dead on both for the same reason,
and it is a shared gap rather than a comparative one. (Cordial has since
implemented it, and voice still does not join; see the voice section above.)

### One feature they have that Cordial cannot have

mocktail feeds Roblox's own in-game audio-output picker a live device list by
**vtable-patching `FmodAudioDevice` inside the mapped `libroblox.so` image** —
`mprotect` on the RELRO range and writing over the output-selection methods,
`src/audio/roblox_output_device_bridge.cc:120`.

That is precisely what [ADR-001](docs/adr/ADR-001-in-process-hooking.md) and
[ADR-003](docs/adr/ADR-003-plugin-isolation.md) put permanently out of scope: no
hooking, no memory patching of the Roblox process, *absent* rather than disabled.

So the user-visible difference is real and will not be closed by implementing
something. A mocktail user who opens Roblox's audio-output menu sees it
populated; a Cordial user does not. Cordial's output follows the host default
through PipeWire's `PW_ID_ANY` and autoconnect instead, so switching devices
works — through the system mixer rather than from inside Roblox's settings.

Worth stating in those terms rather than as a gap, because someone will
eventually file it as a bug and the answer is a design decision, not an omission.

---

## Updater and symbol resolution, compared against mocktail

Four real gaps, ranked. The first two are the same failure this project hit on
2026-08-18 when Roblox shipped 2.734.0.917 and the client stopped loading.

### U1. A new libm symbol is a manual code change. It should not be.

**Fixed, 2026-09-13 --- see [ADR-034](docs/adr/ADR-034-symbol-resolution-asks-the-library.md).**
`symtab::build` now takes the engine's own imports, read out of the ELF, and
resolves the union of those and `SYMBOLS`. The diagnosis below was right and is
kept for it.

**This is the highest-value finding of the comparison.** `hypotf` stopped the
client loading entirely, and the fix was a hand-edited row in a TSV.

The resolver was never the problem. `symtab.rs`'s `lookup` does `host_dlsym` and
then confirms the *defining* object is the library asked for — it would have
resolved `hypotf` from host libm on the first try. What gates it is that
`symtab.rs:205` iterates `SYMBOLS`, generated by `build.rs` from
`docs/analysis/undefined-symbols.tsv`, so a name absent from that file is never
looked up at all. The capability was there; the allowlist was not.

mocktail sidesteps the whole class. `stubs/CMakeLists.txt` builds an empty
`.so` per library and force-links the real host one into it:

    target_link_options(${_target} PRIVATE "LINKER:--no-as-needed")
    target_link_libraries(${_target} PRIVATE -l${_link})

so every symbol libm or libz has ever exported resolves without anyone editing
anything.

**libc must stay curated and that is not part of this.** Both projects hand-pick
libc deliberately, because bionic and glibc disagree on `sigset_t`,
`struct sigaction`, `pthread_mutex_t` and `struct addrinfo`, and passing those
through unchanged overruns the caller's object. The proposal is libm and libz
only, where the ABI is IEEE arithmetic and byte buffers and there is nothing to
translate.

- **Touches:** `crates/cordial-runtime/src/symtab.rs`. Optionally `build.rs` if
  the tsv is regenerated from a readelf diff in CI instead.
- **Scope:** moderate. Isolated.
- **Verified, not assumed:** `lookup`'s body and the `SYMBOLS` loop were both read.

### U2. Nothing validates a new Roblox build before adopting it, and there is no way back

`justfile` re-extracts `libroblox.so` whenever the APK changes and overwrites the
cache. `cordial-update`'s `cache.rs` is stamp bookkeeping and holds no verdict.
So a build that does not load replaces one that did, and the previous engine is
gone.

mocktail runs its candidate twice before promoting it — `RunCandidateCanaries`,
checking real Vulkan queue presentation and clean audio buffer consumption — and
reinstalls the last pinned-good build if the new one fails probation.

That is exactly this week's failure: the client silently stopped loading until a
human noticed and hand-patched a symbol. With U1 fixed the specific cause goes
away; the shape does not.

- **Touches:** `crates/cordial-update/src/cache.rs` (a verified gate), the
  `justfile`'s extraction (keep the prior copy on failure).
- **Scope:** moderate.

### U3. The APK's signature is never checked

`cordial-update/src/apk.rs` treats the archive as a hostile zip — path traversal,
symlinks, mode bits — and never asks who signed it. Cordial then executes
whatever `lib/x86_64/libroblox.so` is inside a file the user pointed at.

mocktail verifies APK Signature Scheme v2/v3 against pinned signer fingerprints
before trusting an extraction. This is not vendoring anything; it is validating a
file already on the user's disk.

- **Touches:** `crates/cordial-update/src/apk.rs`. **Scope:** moderate.

### U4. Packaging reach

mocktail ships AppImage, DEB, RPM, pacman and three AUR variants alongside
Flatpak. Cordial has Flatpak. No breakage, narrower reach. **Scope:** major.

### Deliberate divergences, no action

**Where the APK comes from.** mocktail downloads from APKPure. `download.rs`
refuses that route by design — Roblox publishes no Android URL, and a mirror's
own hash is the mirror agreeing with itself. U2's validation machinery is
adoptable without touching this, since it does not care where the file came from.

**The cache stamp** is size, mtime and path rather than a content hash, which
avoids rehashing 115 MB every launch and still catches a swapped build. Keep.

**Neither project has update channels.** Grepped both.

---

## From mocktail's commit history: one bug we share, one landmine

All 41 commits read with diffs. A fix commit is a bug somebody actually hit,
which makes this a better-yielding search than diffing current source — most of
what follows is "already handled" or "not applicable", and that is the useful
shape of the result.

### C1. A rejected login cookie fails silently and the user becomes a guest

mocktail's `0b8cbef` added a dialog when Roblox answers 401 to a saved cookie.
Before it, the session died and the client fell back to guest sign-in with only a
line on stdout.

**Cordial has the same hole.** `cookies.rs` restores saved cookies mechanically
and has no rejection handling; the only mention of 401 in the whole file is a
comment at line 578 about ordering. Nothing in `cordial-shell` surfaces an
expired session. So a user whose `.ROBLOSECURITY` has aged out is quietly signed
out and finds out by noticing they are a guest.

The shell already has somewhere to put this — `window.rs` and `updater.rs` both
raise user-visible notices.

**One caveat that makes this harder than mocktail's version, and it should be
settled before anyone starts:** Cordial does not make the request that gets the
401. The engine does its own HTTP, so there is no response for the cookie module
to inspect. Either the signal comes from the engine's own log, or it comes from
the web view once that path carries sign-in, or Cordial observes it somewhere not
yet identified. **Find the observation point first.** Wiring a dialog to a
condition nothing can detect is worse than the silence.

- **Touches:** `crates/cordial-runtime/src/cookies.rs`, plus a notice in
  `cordial-shell`. **Scope:** moderate, once the detection point is known.

### C2. A landmine for whoever implements `pthread_create`

mocktail's `23fe2ee` fixes glibc's static TLS eating into a guest thread's
requested stack — a thread asks for N bytes and gets meaningfully less, and the
overflow lands somewhere unrelated.

**Not reachable in Cordial today**: `pthread_create` is not in
`bionic::pthread::overrides()` at all — grepped, zero occurrences. So this is not
a live bug. It is the first thing to get wrong when guest thread creation is
implemented, and it is recorded here so that person does not discover it the
expensive way. `INFERRED`: taken from their fix, not reproduced here.

### Verified as not-present, so nobody re-checks

- **`dlmopen` namespace collision** (`6aa22c2`). Theirs happened because a host
  `dlopen`'d library bound to the executable's own exported bionic-compat
  symbols. Cordial exports none — `readelf --dyn-syms` on both binaries shows no
  libc-name collisions, and Roblox's imports resolve through a private in-process
  table rather than the loader's global scope.
- **LTO folding FORTIFY wrappers into self-recursion** (`52721bf`). Theirs
  redefines `__memcpy_chk`-style names that collide with glibc's. Cordial's
  equivalents are plain Rust that never carry a `__*_chk` symbol name and call the
  unfortified host function. No name for LTO to collide on.
- **Canary and first-install approval bugs** (`3ce39c1`, `63070b6`). Bugs inside a
  build-approval system Cordial does not have. The adjacent real gap is already
  U2 above.

### Already ahead

The low-quality-texture flag (adopted), user-editable FastFlag overrides (Cordial
has layered ones with user-wins, predating theirs), and the Discord join-button
fix (nothing to attach it to — Cordial's presence plugin has no party-secret
concept).

### Not applicable

SDL3/CMake linkage, the FreeBSD port, system-proxy auto-detect,
`--force-run-latest`, cross-launcher cookie borrowing they added and then removed,
their logging rewrite, and twenty-two packaging, CI, AUR, release and website
commits. One line each is the correct treatment.

---

## Two input bugs, with mechanisms and the diagnostic for each

Both reported from real play, both with a named candidate mechanism and neither
confirmed. Written down because guessing at input in this file has cost time
before.

### I1. WASD dead when launched from a job, working when launched interactively

Reported: launched through the join script, camera and space work and WASD does
not. Launched interactively and used normally, everything works. **The difference
is in how it starts, not in-game state** — which rules out the first theory
(a focused text box swallowing the letters) because that would not care how the
process was started.

**The candidate.** `wayland.rs`'s `dispatch_key` delivers each key down *two*
paths, and only one is conditional:

```rust
if handle != 0 {
    super::input::deliver_key(handle, down, keycode, ...);   // AGDK
}
super::input::pass_key_event(down, evdev_key as i32, meta);  // NativeInputInterface
```

`handle` is `active_handle`, stored by `WaylandWindow::pump`. And `load.rs`
already documents a startup path that pumps with `None`:

    // No AGDK handle on this path — it drives the app bridge directly and
    // never calls initializeNativeCode, so onTouchEventNative etc. are never
    // registered to deliver input to.

So there is a real, already-known configuration in which the AGDK half of key
delivery does not happen and the `NativeInputInterface` half does. If Roblox
routes character movement through one and jump through the other, "space works,
WASD does not" is exactly what that looks like.

**Not confirmed**, and one thing argues against it: that documented path is the
app-bridge one, and the join script passes `--game-activity`, which should take
the other. So either the handle is zero for a different reason on that path, or
the mechanism is elsewhere. **Do not change the input code on this until the
trace says which.**

    CORDIAL_TRACE_TEXT=1 tools/join-run.sh input 2>&1 | grep 'wayland key'

Every line ends with `focus=…`. Run it once reproducing the bug and once not, and
compare — whether keys arrive at all, whether `focus` differs, and whether the
two runs take different pump sites.

### I2. Camera sensitivity spikes through a full revolution, then settles

Reported: turning the camera a full revolution makes it briefly over-sensitive
and then correct itself. The user's own guess — the cursor moving and being
reset — is close to the mechanism the code already guards.

`wayland.rs:2023` bails out of `dispatch_relative_motion` unless
`POINTER_LOCK_ACTIVE`, and says why: *"acting on it unlocked would double every
ordinary mouse movement."* So the doubling is known and defended against.

**The candidate is a race on that flag.** `sync_pointer_lock` runs once per pump,
so there is a window in which the flag and the compositor's actual lock state
disagree, and in that window both the absolute `wl_pointer.motion` path and the
relative path deliver for the same physical movement. Doubled input, then it
settles — which is the reported symptom, and a full revolution is when a lock
boundary is most likely crossed.

**Not measured.** The confirmation is two `nativePassMouseMove` calls for one
movement during the spike:

    CORDIAL_TRACE_MOUSE=1 tools/join-run.sh camera 2>&1 | grep nativePassMouseMove

If they appear in pairs while the sensitivity is wrong, that is it. Worth having
before touching the lock logic — this file has a history of fixes that addressed
the wrong half.

---

## The untextured load is not the texture manager. It is the missing content store

Run side by side on the same place, same session, same account:

| run | manager the engine chose | asset failures |
|---|---|---|
| default | **TM2** | 6 |
| `...DenyPattern2=".*"` | **TM1** | 2 |
| default, the untextured one reported from play | **TM2** | 12 |

**The run that came out untextured was on TM2.** So denying TM2 was never the
cause of the good case and TM1 was never the cause of the bad one — the same
manager produces both. That closes the line of questioning `dc650e5` opened, and
it means the flag was doing nothing useful in either direction.

### What the failures actually are

Not textures at all:

    Asset (Image) "rbxthumb://type=AvatarHeadShot&id=&w=48&h=48..." load failed:
      Error parsing batch thumbnail request

**`id=` is empty**, and the first reading of that here was wrong. It was filed as
an identity-propagation bug in Cordial on the strength of the empty field alone.

It is not. Reported by the account's owner: Roblox's own servers lost that
avatar's head-shot, and it renders blank on *every* client, Cordial or otherwise.
So the empty id is Roblox asking for a thumbnail that no longer exists, and the
parse failure is the honest downstream consequence.

Worth keeping only as a caution: an asset error naming an image, in a session that
looks untextured, is very easy to read as the cause of the untextured world. It
was not, twice over — the world's textures are the content store, and these
particular failures are somebody else's deleted file.

### The variance has an obvious cause and we have been chasing it all session

Reported: *"sometimes the loading screen is really slow and the game loads
everything, but another run may load really fast but the textures aren't loaded
and everything is still loading."*

That is precisely what a client with **no content store** looks like.
`RbxStorage::init` never runs, so nothing is cached between sessions or within
one, and every asset is fetched from the network every time. Load behaviour is
then entirely at the mercy of the network and of whatever order the engine
happens to ask in — fast and incomplete, or slow and complete, with nothing in
between and no reason for either.

The same log carries `RbxStorage is not initialized` twice while this was
happening.

So the storage gap is not an abstract completeness issue to be closed one day.
**It is the cause of the loading behaviour a player actually notices**, and that
reframes its priority: it was being tracked as a possible cause of the 304, which
§14 weakened, and it should be tracked as the cause of this instead, which is
observed.

### What this does not explain

Nine frames per second in that session, with 188 ms ping. Untextured geometry
should be cheaper to draw rather than more expensive, so the frame rate is a
separate fact and AGENTS.md's rules about measuring it apply — nothing here drove
input for the measurement, so 9 fps is a number off a HUD and not a result.
