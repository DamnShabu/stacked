# NVIDIA, TextureManager2, and "shader translation": what was actually asked, and what was measured

The request handed into this investigation was "shader translation for NVIDIA
so texture manager v2 works." No NVIDIA hardware exists on this host (Intel
RPL-P integrated graphics only), so this is a record of what could be
established without it, and where the trail runs out.

## The request does not match anything on file

Neither Cordial's own issue tracker (`gh issue list --state all`, 51 issues,
searched for `nvidia`/`texture`/`tm2`/`shader`/`astc`/`etc2` in title and body)
nor `docs/NEXT.md` names an NVIDIA-plus-texture report. The only NVIDIA item on
record is TASKS.md's **T2**, and it is not about textures:

> `FStringGraphicsVulkanShaderMTDenyPattern` ... mocktail sets this to
> `"4318:.*"`. 4318 is `0x10DE`, the NVIDIA vendor ID, so this disables Vulkan
> shader multithreading on all NVIDIA parts. ... Caveat: unverifiable on this
> machine ... Ships as `INFERRED` unless someone with NVIDIA hardware confirms
> it.

T2 is a threading workaround for the engine's own shader *compiler*, not a
texture format concern, and it does not touch TextureManager2 at all. It is
the closest thing on file to "NVIDIA + needs a tester," and it is plausibly
what got compressed into "shader translation for NVIDIA" somewhere upstream of
this task. Treat that as a hypothesis about where the request came from, not as
something this document confirms — nobody traced the exact wording back to a
person.

**TextureManager2 itself already has a long, measured history here, and it
argues against the premise.** `crates/cordial-runtime/src/flags.rs` (around
line 390) and `docs/analysis/flag-init.md` §20 record that Cordial once shipped
`FStringGraphicsTextureManager2DenyPattern2=".*"` as a default — forcing the
engine onto the legacy TM1 — copied from a mocktail commit titled "Another fix
for low quality texture." It was removed because:

- the blurring it was meant to fix **was never once observed on Cordial**, by
  anyone, on any hardware;
- forcing TM1 broke lobby rendering and slowed loading, measured from real
  play;
- a side-by-side (TASKS.md, "The untextured load is not the texture manager")
  found the *same* manager, TM2, behind both a textured run and an untextured
  one on the same account and place — so TM1-vs-TM2 was never actually
  correlated with the symptom it was blamed for.

## Sober already has this exact history, independently

Sober runs the same engine on the same class of desktop hardware and has a
9-year-deep public issue tracker (`tools/sober-corpus/data/raw.jsonl`,
2,194 issues). Searched for texture/blurry/TM2/ETC2/ASTC/NVIDIA co-occurrence:

- **#2078** ("Texture override fast flag not working"), maintainer comment:
  > Sober forcefully downgraded Roblox to their TextureManager1 technology,
  > which still supported the texture override fastflag. We had to remove that
  > downgrade because it was breaking the Mesh LOD system.

  Sober tried the identical TM1-forcing flag Cordial once shipped, for the
  identical reason, and reverted it for the identical class of regression
  (something else broke). Two independent projects converged on the same
  finding without sharing code.

- **#1536**/**#1537** ("Sober In game Blurry Textures") — reported with no GPU
  vendor named. A Sober maintainer asked the reporter to test with
  `SOBER_DEBUG_FORCE_NO_NATIVE_ETC2=1`, and the issue was eventually closed
  with: *"This is a Roblox bug, however, we've found a workaround for it
  temporarily that has been applied to all Sober users."* Not NVIDIA-specific,
  not described as a driver-format gap, and the fix was server-side or
  flag-side rather than a client-side transcoder.

- No issue in the corpus ties an NVIDIA GPU to a texture-quality or
  TextureManager complaint specifically. NVIDIA appears constantly in the
  corpus, but for unrelated Vulkan surface/device-loss crashes
  (`vkGetPhysicalDeviceSurfacePresentModesKHR failed`, `VK_ERROR_DEVICE_LOST`,
  "couldn't find a supported graphics device") — a real and large NVIDIA
  problem class on this engine, just not this one.

## What Cordial's Vulkan shim actually does before this session

`crates/cordial-runtime/src/android/vulkan.rs` interposes Roblox's Vulkan
entry points onto the host loader (see its module doc). Before this session it
intercepted exactly five names: `vkCreateInstance`,
`vkEnumerateInstanceExtensionProperties`, `vkCreateAndroidSurfaceKHR`,
`vkCreateDevice`/`vkCreateSwapchainKHR`/`vkGetPhysicalDeviceSurfaceCapabilitiesKHR`
(WSI plumbing), and `vkQueuePresentKHR` (present-mode override and counting).
**Nothing touched format or feature queries, image creation, or shader module
creation** — every one of those calls went straight through to the real host
driver, unmodified. So the premise that Cordial needs to *translate* anything
at the Vulkan boundary for texture formats was, before this session, untested
in both directions: nobody had shown the engine asks a format question Cordial
would need to answer differently, and nobody had shown it doesn't.

TASKS.md's own "detex: premise unproven" section named exactly this gap:

> Vendoring detex before knowing that is the same mistake mimalloc nearly was.
> **The measurement first:** one join with the graphics log turned up,
> recording which `VK_FORMAT_*` the engine requests and whether any are
> refused.

That measurement had never been taken for Cordial. This session takes it.

## What was added, and what it measured

`vulkan.rs` gained a passthrough, counting interposer on
`vkGetPhysicalDeviceFormatProperties` (logs/counts the format family queried —
ETC2, ASTC, BC, or other — and whether the driver's answer is fully
unsupported) and on `vkCreateShaderModule` (a call count). Both are wired into
the existing `CORDIAL_COUNT_GL=1` report and `CORDIAL_ANDROID_TRACE=1` tracing
that `glcount.rs` and `android::trace` already provide for other graphics
calls — no new mechanism, same switches. Neither can change engine behaviour
by itself; see `vk_get_physical_device_format_properties`'s doc comment.

A second, explicitly test-only capability was added and gated behind
`CORDIAL_MASK_MOBILE_TEXTURE_FORMATS=1` (off by default, never set by any
shipped configuration): when set, the same interposer reports ETC2 and ASTC as
**fully unsupported** to the engine regardless of what the real driver says.
This substitutes for NVIDIA hardware this project does not have. See
[ADR-042](../adr/ADR-042-texture-format-query-observability.md) for why this
lives in Cordial's own shim rather than being a permanent behaviour change,
and for the ADR-001/003 boundary argument.

**Why not point the client at real desktop-GPU-shaped hardware instead of
masking?** This host has exactly one Vulkan device other than the Intel iGPU:
Mesa's `llvmpipe` software rasterizer, which genuinely reports
`textureCompressionETC2`/`ASTC_LDR` = `false` (`vulkaninfo`, below) — the same
signature real discrete GPUs typically have. That looked like a free, honest
substitute and was tried first. It does not work: `MESA_VK_DEVICE_SELECT`
reorders `vkEnumeratePhysicalDevices`'s result but does not filter it (checked
directly with a five-line C program against the loader, not inferred from
`vulkaninfo --summary`'s misleadingly single-device output), and Roblox's own
device-selection code rejects `llvmpipe` outright before any format query is
reached:

```
[FLog::Graphics] Vulkan: Device llvmpipe (LLVM 22.1.8, 256 bits) is emulated, skipping
```

printed identically whether `llvmpipe` is enumerated first or second. So this
host cannot make the real client run against a real driver lacking these
formats; the mask is the only available lever, and its output is scoped
accordingly below.

## Measurements

All runs: `target-toolbox/release/cordial-run --host-libc --game-activity
--profile tex`, signed-out landing UI (no account, no game join — the daily
signed-in test cap was already used today), `XDG_DATA_HOME` pointed at a
throwaway root, a nested headless `sway` under `distrobox enter cordial` on
its own `WAYLAND_DISPLAY`, `CORDIAL_COUNT_GL=1 CORDIAL_ANDROID_TRACE=1`. Built
at this session's
HEAD (`crates/cordial-runtime/src/android/vulkan.rs` and `glcount.rs` changes
below), on top of commit `38528b8` (`9d43ce2` plus two unrelated packaging
commits the maintainer made concurrently on this shared host — see the
session's commit log; neither touches `crates/`).

### Host driver capability (`vulkaninfo`, Intel RPL-P, Mesa 26.1.6)

```
VkPhysicalDeviceFeatures:            GPU0 (Intel, real)   GPU1 (llvmpipe)
  textureCompressionETC2               true                 false
  textureCompressionASTC_LDR            true                 false
  textureCompressionBC                  true                 true
```

Intel's Xe-LP-class hardware decodes ETC2 and ASTC natively; it is not
representative of NVIDIA, which is why the mask exists.

### Baseline (real Intel driver, no mask)

```
vkGetPhysicalDeviceFormatProperties(ETC2)   0
vkGetPhysicalDeviceFormatProperties(ASTC)   6
vkGetPhysicalDeviceFormatProperties(BC)     4
vkGetPhysicalDeviceFormatProperties(other) 22
  ...of which unsupported (both tiling feature masks zero)   0
vkCreateShaderModule                     2211
```

Engine log: `Using TM2 in MipPackStream mode`, no `Using TM1` anywhere, no
Vulkan error.

**The engine queries ASTC directly, at the signed-out landing screen, before
any game is joined.** That settles TASKS.md's prerequisite: Cordial's build
does exercise the compressed-format query path, not only a code path that
assumes support. It never queried ETC2 in this window — worth keeping open
rather than closing: the landing UI may simply have no ETC2-encoded asset in
it, or the engine may gate ETC2 on the `VkPhysicalDeviceFeatures` boolean
instead of a per-format query (not instrumented this session — see below).

### Masked (`CORDIAL_MASK_MOBILE_TEXTURE_FORMATS=1`: ASTC forced to zero support)

```
vkGetPhysicalDeviceFormatProperties(ETC2)   0
vkGetPhysicalDeviceFormatProperties(ASTC)   2
vkGetPhysicalDeviceFormatProperties(BC)     4
vkGetPhysicalDeviceFormatProperties(other) 22
  ...of which unsupported (both tiling feature masks zero)   2
vkCreateShaderModule                     2205
```

Confirmed masked in the trace: `vkGetPhysicalDeviceFormatProperties(format=157,
family=astc): MASKED to unsupported`. Engine log, same run:

```
[channel] Caps: Texture: DXT 1 PVR 0 ETC1 1 ETC2 1 Half 1
[FLog::Graphics] TM2: retry:false
[FLog::Graphics] TM2: maxCloudAssetDimension: 4096
[FLog::Graphics] Using TM2 in MipPackStream mode
```

**Unchanged.** With the one format query the engine actually makes (ASTC)
answered as fully unsupported, TM2 selection, the shader count (2205 vs 2211 —
within the run-to-run noise this landing screen already shows across the
unmasked baselines, 2211 vs 2339 on the earlier llvmpipe-reorder run), and the
engine's own internal texture capability line (which still claims `ETC2 1`
regardless — that line is not sourced from this Vulkan query, since ETC2 was
never queried through it in any run) all look identical to the unmasked run.

**This falsifies the specific causal chain in the request as tested.** At
renderer bring-up, telling the engine ASTC is unsupported does not push it
onto TM1, does not change the shader count materially, and does not surface an
error. Whatever "shader translation... so texture manager v2 works" was meant
to describe, it is not "the engine needs to be told the truth about ASTC
support before it will pick TM2 correctly" — TM2 runs identically whether that
truth is told or hidden, at least for everything a landing screen exercises.

## What this does not settle

- **In-game texture streaming was not measured.** TM2's *decision to stream a
  particular texture at a particular quality* is a live-world concern; a
  signed-out landing screen loads UI chrome, not place content. The daily
  signed-in test-account cap was already spent before this investigation
  started, so a real join was out of reach today. If TM2 behaves differently
  once real assets stream, that would need a joined run to see — flagged as a
  blocker for tomorrow, not silently skipped.
- **`vkGetPhysicalDeviceFeatures`/`Features2`'s `textureCompressionETC2`/
  `ASTC_LDR`/`BC` booleans were not instrumented.** The per-format
  `vkGetPhysicalDeviceFormatProperties` path was chosen because its argument
  and result are simple to decode safely (a `u32` format id in, three `u32`
  bitmasks out); the `Features` structs carry 55 tightly-packed booleans whose
  offsets would need verifying against this container's own `vulkan_core.h`
  before touching them, which was not done this session. If the engine's TM2
  gate (if one exists at all) reads the `Features` booleans instead of asking
  per-format, this measurement would not see it change. **Given the masked run
  otherwise looks identical to baseline, this is not the direction the
  evidence points, but it is a real gap, not a closed one.**
- **`vkCreateImage` was not intercepted.** Whether the engine ever asks Mesa
  to create an image in a format the *driver itself* refuses (as opposed to
  what a query says) is a different, unmeasured question — and on this host
  the real driver supports everything Roblox asks for, so there is nothing to
  observe here regardless.
- **Real NVIDIA hardware behaviour remains `INFERRED`.** Everything above is
  either directly measured on Intel, or measured against a masked answer this
  session invented to stand in for NVIDIA. Cordial's own device-selection log
  line and driver report on a real NVIDIA part could differ from both.

## Conclusion

No fix was built. The premise handed into this investigation — that TM2 needs
some form of "shader translation" on NVIDIA to work correctly, implicitly
tying together T2 (an unrelated Vulkan-threading workaround) and the
already-reverted TM1-forcing flag — does not survive contact with either this
project's own measured history or a direct experiment built to test it this
session. Building an ASTC/ETC2 transcoder (TASKS.md's T3) on the strength of
this request would have repeated the exact mistake `flags.rs`'s own comment
already documents: shipping a remedy for a symptom nobody has actually
reproduced on Cordial, on the strength of another project's flag title.

What was kept: a small, off-by-default observability addition
(`vkGetPhysicalDeviceFormatProperties`/`vkCreateShaderModule` counting) that
answers a question TASKS.md explicitly left open, and a test-only masking
switch for whoever next has NVIDIA hardware to point at this. Both are
documented in [ADR-042](../adr/ADR-042-texture-format-query-observability.md).
