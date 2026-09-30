# Security policy

## Reporting

Report security issues through GitHub's private vulnerability reporting on this
repository ("Security" → "Report a vulnerability", or
[directly](https://github.com/DamnShabu/stacked/security/advisories/new)).
Please do not open a public issue for anything exploitable.

Include what you did, what happened, and what you expected. A reproduction that
someone else can run is worth more than a description, and this project's whole
method is that claims are verified by running them.

## Supported versions

Only the newest release is supported, and a fix ships in the next one. Your
package manager, or the Roblox manager's *Update Stacked*, installs it.

| Version | Supported |
|---|---|
| 0.21.1 (the newest) | Yes |
| 0.21.0 and older, and every Cordial release | No — update first |

## What counts

Stacked runs a large proprietary binary inside a runtime it supplies, and hosts
plugins alongside it. In scope:

- **Sandbox escape from a plugin** — a plugin reaching Stacked's memory, the
  Roblox process, or the host beyond its granted capabilities. Plugins are
  isolated by process ([ADR-003](docs/adr/ADR-003-plugin-isolation.md)); a way
  around that is the most serious class of bug here.
- **Capability broker bypass** — obtaining an effect without the capability that
  should gate it.
- **Anything in the runtime that lets untrusted content reach the host** — path
  traversal out of the asset tree, the `/system` redirect
  (`native/system_paths.cpp`) resolving somewhere it should not, or the flag
  layering reading a file it should not.
- **Memory-safety bugs in Stacked's own `unsafe` code**, most of it inherited
  from Cordial, of which there is a great deal: the linker bindings, the
  bionic shims, and the libc interposers in `native/`.

## What does not

- **Crashes in Roblox's own code.** The engine is not ours and most crashes are
  Stacked handing it something malformed. Those are ordinary bugs — open a
  normal issue.
- **The ban risk from using a third-party client.** That is documented in the
  README, is inherent to what this is, and is not a vulnerability.
- **Requests for an exploit surface.** Stacked deliberately has no script
  execution and no hooking
  ([ADR-001](docs/adr/ADR-001-in-process-hooking.md)). Asset overlays exist,
  non-destructively and off by default
  ([ADR-010](docs/adr/ADR-010-plugin-asset-overlays.md), which superseded
  ADR-004's refusal). "Stacked cannot cheat" is the design, not a bug.

## Forks, and clients built on Stacked

Stacked is itself a fork of Cordial, and it keeps Cordial's position on this
unchanged. It does not support script execution, exploiting, botting or
multi-accounting, and it will not. That is not a gap waiting to be filled.
[ADR-001](docs/adr/ADR-001-in-process-hooking.md) makes in-process hooking,
memory patching and injected script environments **absent** rather than
disabled — there is no primitive here to switch on, and no API by which a plugin
could ask for one. [ADR-003](docs/adr/ADR-003-plugin-isolation.md) is why a
plugin never receives a socket, a file descriptor or a connection: it sends a
payload and Stacked performs the effect.

**Stacked is GPL-3.0, so anyone may fork it, including in directions we
disagree with.** That is the licence working as intended and we are not going to
pretend otherwise. It does mean:

- A fork is an independent project. It is not endorsed by us, not affiliated
  with us, and not supported here.
- If you are using something built on Stacked or Cordial that adds script
  execution, **you are not using Stacked**, and this issue tracker cannot help
  you. We do not
  know what that fork changed and we cannot reason about its behaviour.
- This project will not accept commits that enable exploiting. Contributing here is
  not a route to getting one merged. Nothing happens *to* you for having
  contributed — this is a statement about patches, not about people.

If you are considering using such a fork, understand what you are accepting.
Roblox's enforcement is automated, runs in waves, and associates accounts sharing
an address. A fork that adds an exploit surface does not carry the risk alone;
it carries it into every account on your network.

## WSL is not a supported target

Running Stacked under WSL sidesteps client integrity checks, and that is the
reason it is unsupported rather than merely untested. We will not help with WSL
issues and will not take patches that exist to make that path work.

This is not a judgement about Windows. It is that the value of running there is
mostly the evasion, and building for it would make this project a tool for
something it has said it is not.

## Expectations

This is a hobby project with no funding and no on-call. There is no bounty and
no response-time guarantee. It is also young and largely written by an AI with a
human directing architecture — see the note at the end of
[CONTRIBUTING.md](CONTRIBUTING.md) — so treat its security posture as unproven
rather than assumed.

What a reporter can expect is a best effort at:

<!-- TODO(maintainer): confirm or change these targets. -->

| | Target |
|---|---|
| Acknowledging a report | TODO(maintainer) |
| A first assessment | TODO(maintainer) |
| A fix released, for a confirmed issue | TODO(maintainer) |
