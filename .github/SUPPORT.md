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

## Security issue

Report it privately through [a GitHub security
advisory](https://github.com/DamnShabu/stacked/security/advisories/new)
rather than in a public issue.

## Before you file anything

[`docs/NEXT.md`](../docs/NEXT.md) says what already works, what is blocking,
and what has already been ruled out — including a fair number of things that
looked like bugs and were not. It is worth a look before writing a report
from scratch.
