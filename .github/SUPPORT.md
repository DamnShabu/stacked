# Support

## Report a bug, ask for a feature, or write down a finding

[Open an issue](https://github.com/DamnShabu/stacked/issues/new/choose) and
pick the template that matches. Every one of them asks for the same
**Diagnostics** block — run `stacked diagnostics` (`target/release/stacked
diagnostics` from a checkout, or `flatpak run io.github.damnshabu.Stacked
diagnostics` under Flatpak). It carries the Stacked and Roblox build, your
kernel and distribution, and how Stacked was installed — no account, no
token, no profile name, and nothing from your home directory. It prints to
the terminal and goes nowhere else, so read it before you paste it.

Five templates, routed by shape:

| Template | Use it when |
|---|---|
| **Bug report** | Something behaves differently from what you expected |
| **A Roblox feature does not work** | Something Android would answer that Stacked silently does nothing for |
| **Roblox updated and something broke** | A new Roblox build fails to load, or reaches for a symbol Stacked does not have |
| **Feature or capability** | Something Stacked, or a plugin, should be able to do |
| **Finding** | Something you established about the engine, including something that turned out to be wrong |

There is no blank-issue option — a report without the shape one of these
gives it is much harder to act on, and the templates exist so nobody has to
guess what to include.

## Issue or discussion

**An issue is for something that should change**: a bug, a feature, a document
that is wrong, a finding. **A question that is not yet any of those** — "is
this expected?", "does anyone run it on X?" — belongs in
[Discussions](https://github.com/DamnShabu/stacked/discussions): Q&A for
questions, Ideas for something not yet a feature request.
<!-- TODO(maintainer): the Discord bridge (ADR-030) files issues from a Discord
     server, but no invite link is written down anywhere in the repository.
     Add it here if the server is public. -->

If you are not sure, open the issue. Moving one is cheap.

## Security issue

Report it privately through [a GitHub security
advisory](https://github.com/DamnShabu/stacked/security/advisories/new)
rather than in a public issue.

## Before you file anything

[`docs/NEXT.md`](../docs/NEXT.md) says what already works, what is blocking,
and what has already been ruled out — including a fair number of things that
looked like bugs and were not. It is worth a look before writing a report
from scratch.

## Common questions

**Something is broken. What do I run first?** `stacked doctor`. It checks the
display, Vulkan, sound, the keyring, the browser handler and the Roblox build,
and says what to do about anything it finds. The README's
[Troubleshooting](../README.md#troubleshooting) covers the usual answers.

**Is my bug already known?** [`docs/known-issues.md`](../docs/known-issues.md)
has the state of each one, including those reported against Cordial that apply
here too.

**Will this get my account banned?** It can. Roblox does not support
third-party clients and bans in waves, including by mistake. Do not use an
account you care about.

**Can Stacked run scripts or cheats?** No, and it will not. See
[SECURITY.md](../SECURITY.md#forks-and-clients-built-on-stacked).

More in the README's [FAQ](../README.md#faq).
