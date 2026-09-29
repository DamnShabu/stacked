# ADR-042: Vulkan texture-format queries are counted and, test-only, maskable — nothing is translated

**Status:** accepted
**Supersedes:** nothing
**Related:** [ADR-001](ADR-001-in-process-hooking.md), [ADR-003](ADR-003-plugin-isolation.md), [ADR-034](ADR-034-symbol-resolution-asks-the-library.md), [ADR-040](ADR-040-the-engine-already-runs-mimalloc.md)

## Decision

`crates/cordial-runtime/src/android/vulkan.rs` gains two new interposed Vulkan
entry points:

- `vkGetPhysicalDeviceFormatProperties` — always passthrough, always counted
  (which format family was asked about: ETC2, ASTC, BC, or other; whether the
  driver's answer is fully unsupported), reported through the existing
  `CORDIAL_COUNT_GL=1`/`glcount` mechanism and traced through the existing
  `CORDIAL_ANDROID_TRACE=1` switch.
- `vkCreateShaderModule` — a passthrough call counter, same two switches.

A third behaviour, `CORDIAL_MASK_MOBILE_TEXTURE_FORMATS=1`, makes the format
query above report ETC2 and ASTC as **fully unsupported** regardless of the
real driver's answer. It is off by default, documented in `--help`, and no
build, packaging script, or default configuration sets it.

**No transcoder, no format translation, and no change to what any shipped
client tells the engine was built.** See
[docs/analysis/nvidia-texture-manager2.md](../analysis/nvidia-texture-manager2.md)
for why: the premise that motivated this work — "shader translation for
NVIDIA so texture manager v2 works" — did not survive the measurement this
capability exists to take.

## Why this is Cordial's own code, not a boundary violation

ADR-001 and ADR-003 rule out in-process hooking and memory patching of the
engine, permanently. Nothing here does either. `vulkan.rs` already interposes
Vulkan at the API boundary for five other entry points — `vkCreateInstance`,
`vkCreateAndroidSurfaceKHR`, `vkCreateDevice`/`vkCreateSwapchainKHR`/
`vkGetPhysicalDeviceSurfaceCapabilitiesKHR`, and `vkQueuePresentKHR` — because
that boundary is Cordial's own dispatch table (`vkGetInstanceProcAddr`'s
result), returned to the engine by Cordial's own code, not a patch applied to
`libroblox.so`. The engine calls a function pointer Cordial handed it; Cordial
answers the call, forwards to the host's real implementation, and returns
whatever the host returned (unmodified, in the counting case; a controlled
false answer, only under the opt-in mask). At no point does this read or write
the engine's memory, alter its instructions, or run code inside its address
space — it is argument/return translation at an API boundary Cordial already
owns, the same class of thing the module's existing WSI shims already do, and
explicitly the boundary this task's brief asked to be argued rather than
assumed.

The mask is the same category of thing again, narrowed to a test-only switch:
it changes what Cordial's own shim *says*, in response to a query the engine
makes of Cordial's own dispatch table, never what runs inside the engine. It
is the Vulkan-boundary equivalent of `CORDIAL_NO_VULKAN=1` (already
shipped, in the same file's `--help` block) telling the engine the host has no
Vulkan loader at all — a controlled, logged, opt-in false answer at a boundary
Cordial already owns, used to observe a fallback path this host cannot
otherwise reach.

## Why counting, not just tracing

`glcount.rs` already exists for exactly this shape of question — "did the
engine actually call X", not "did it look X up" (the `vkGetInstanceProcAddr`
resolution list is identical across backends regardless of what actually gets
called; see that function's own comment). Reusing it rather than adding a
second ad-hoc reporting mechanism keeps one place that answers "what did the
graphics session actually do" after `--run`, and it was already carrying
Vulkan's `vkQueuePresentKHR` alongside the GLES counters for the same reason.

## Why the mask, and why it is not a default

No NVIDIA hardware exists on this project's development host. The two options
for exercising an NVIDIA-shaped format-support gap without it were: point the
real client at a real driver that also lacks ETC2/ASTC, or have Cordial's own
shim lie about it. The first was tried and does not work on this host — Mesa's
`llvmpipe` software rasterizer genuinely lacks both, but Roblox's own
device-selection code rejects it outright (`Vulkan: Device llvmpipe ... is
emulated, skipping`) before any format query is reached, and
`MESA_VK_DEVICE_SELECT` only reorders `vkEnumeratePhysicalDevices`'s result
rather than filtering it (verified directly against the loader, not inferred
from a tool's summary view). That leaves the mask as the only lever this host
has, so it exists — gated behind an environment variable nothing sets by
default, logged whenever it fires, and documented in `--help` as a test-only
substitute for hardware this project lacks. It is the same shape
AGENTS.md's rule about test credentials describes for a login form, applied to
a driver capability query instead: a controlled falsehood, told only when
asked for by name, never told by default, and never presented as what a real
NVIDIA driver reports.

## What this does not settle

- Whether real NVIDIA hardware reports the same thing the mask asserts.
  `vulkaninfo` on Intel RPL-P shows `textureCompressionETC2` and
  `_ASTC_LDR` both `true`; NVIDIA desktop parts are widely reported (outside
  this project, not verified here) to report both `false`, which is the shape
  the mask assumes. This remains `INFERRED`.
- Whether TM2's behaviour differs once a real game's textures are streaming,
  as opposed to the landing UI's chrome. Not measured this session — see the
  analysis document's "What this does not settle" for why (the signed-in test
  budget was already spent).
- Whether `vkGetPhysicalDeviceFeatures`/`Features2`'s compression booleans
  matter to whatever gates TM2, as opposed to the per-format query this ADR's
  capability observes. Not instrumented this session.

## What would change this

If someone with real NVIDIA hardware runs `CORDIAL_COUNT_GL=1
CORDIAL_ANDROID_TRACE=1 cordial-run ...` (mask *off*) and the engine's own
format queries or its `Using TM1`/`Using TM2` log line differ from what this
document records for Intel, that is the first real data point this
investigation has been missing, and it would either confirm or retire the mask
this ADR adds. Until then, the mask is the closest substitute available and
nothing more.
