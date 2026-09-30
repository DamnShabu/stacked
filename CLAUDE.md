# CLAUDE.md

@AGENTS.md

The above is the whole contract; everything below is Claude Code specifics.

## You are an orchestrator. Delegate by default

**The main session's scarcest resource is its own context, and investigation
burns it fastest.** Reading a file to find one fact costs the same as reading it
to change something, and a session that spends itself grepping has nothing left
for the judgement only it can make. So the default shape of work here is: the
main session decides, subagents find out, the main session checks and commits.

Delegate: reading and searching, measuring and benchmarking, sweeps across a
parameter space, comparing against another runtime, building and running
anything long, anything that returns a small answer after a lot of looking.

Keep: what to do next, whether a result is believable, what the commit message
says, and every judgement about scope, licence, or an ADR. Those are the things
that need the whole conversation in view, and they are exactly what a subagent
does not have.

A session that ends with "I ran out of context investigating" produced less
than one that ends with three delegated findings and two verified commits.
That happened on 2026-08-22: most of a day went on reading source directly,
and five consecutive theories about one input bug died — while the single
subagent running that day produced the most valuable finding of the session by
reading one file nobody had opened.

### Write the brief as though you will not be asked a follow-up

An agent that has to guess what you meant will guess wrong in the direction
that costs the most. Every brief should carry:

- **The ADRs it needs, by name.** `docs/adr/` is long and an agent that skims it
  will contradict a decision without noticing.
- **Which files not to touch**, explicitly, when work is in flight elsewhere.
  Parallel agents in this repository have collided on `window.rs` and the
  native shims more than once.
- **What has already been ruled out, and by what measurement.** Otherwise it
  re-runs the experiment you already did. Four TaskScheduler hypotheses and a
  poll-coalescing change have each been re-derived by somebody who was not told.
- **The measurement and the control**, named up front. Not "see if it is
  faster" — say what number, taken how, against what baseline with the change
  off. This project has four "fixes" that measured nothing because the
  instrument was chosen after the fact.
- **Its licence boundary**, when it will read another project. mocktail is
  Apache-2.0 and may be adapted with attribution; Nuah carries no licence at
  all and may be read but never copied from; Sober and iceblox are not
  source-available and may only be observed running. The rule is the idea, not
  the transcription, and an agent that is not told will assume the most
  permissive reading.

### Check what comes back

**When a subagent reports success, that is a claim and not a result.** The rule
in AGENTS.md about never stating an unobserved result does not transfer through
delegation — inheriting a claim and repeating it to the user is exactly the
failure that rule exists to prevent, with an extra step that makes it feel
verified.

Ask for pasted output, not summaries. Re-run the one command that decides it.

**And let them check you.** On 2026-08-22 the main session told a subagent that
AGENTS.md documented the wrong CPU core count. It does not — the claim was in
`flags.rs`, and the agent found that by reading the file instead of taking the
orchestrator's word. Brief agents so that contradicting you is obviously
welcome, because the orchestrator is the one participant nobody else is
checking.

### Running several at once

Say plainly what else is running and what it is doing. Two clients rendering at
the same time make every CPU and frame-rate number meaningless, and the rule
that measurements here are sequential is easy to honour and easy to forget.

Tell them the machine is shared with a human who may be playing. A subagent that
kills a running client to free a profile lock has destroyed a live specimen of
an intermittent bug.

**Never match your own command line.** `pkill -f cordial-run` and
`pgrep -f 'cordial.*profile'` both match the shell running them, and both have
killed the session that issued them — five times in one day.

**And `pgrep -x cordial-run` is not the fix, because it does not work.** The
engine renames the main thread, so `/proc/<pid>/comm` reads `Main` and an
exact-name match finds nothing for a client that is in a game with audio
playing. It handed a subagent a false "nothing is running" mid-session, on
2026-08-22, hours after this very paragraph recommended it. Today's own gdb
capture shows the same thing from the other side: `Thread 1 (LWP 1452113)
"Main"`.

Use `pidof cordial-run`, which matches the executable rather than the thread
name, or read it directly with
`os.path.basename(os.readlink('/proc/<pid>/exe'))`.

## Read mocktail before guessing

`~/Projects/mocktail` is a working implementation of most of what this project
is still working out, in plain source, Apache-2.0. **Consult it whenever you are
about to infer a platform contract** — a JNI descriptor, a JS calling
convention, a call ordering, the shape of a payload. It is not a shortcut around
thinking; it is the same class of evidence as `docs/traces/`, and the one rule
at the top of AGENTS.md applies for the same reason.

This is written down because it was learned expensively on 2026-08-20, twice in
one day:

- `ForceNativeFlagsLoadedForTaskScheduler` was read as proof that mocktail
  memory-patches the engine and is therefore no model for Cordial. It is gated
  to one legacy build and inert on ours. That mistake put an entire section into
  `flag-init.md` and three agent briefs before the gate was read.
- The Join button was guessed at twice — first as a bare WebKit message handler
  nothing calls, then as Android's `addJavascriptInterface` shape. The page
  actually wants `window.__globalRobloxAndroidBridge__.executeRoblox(json)`,
  which is in `src/webview/webview_helper_policy.cc` in plain JavaScript and
  took two minutes to find once somebody looked.

Sober is a second working reference and mocktail a third implementation on the
same Build ID, on this host, not inside Android. Where they disagree with an
inference drawn here, they are usually right.

**What may be taken is the idea, not the transcription.** Adapting a documented
shape and crediting it is right; copying an implementation wholesale is not, and
the same line that governs Roblox's binary governs this — see AGENTS.md. Note
also that mocktail's architecture differs in ways that matter: its web view is a
*separate process* forwarding over a socket, so "how mocktail does it" is not
always transplantable, only informative.

## Cut releases, and cut them often

**A change that is not in a release has not reached anybody.** This project spent
six days and 193 commits between `v0.6.0` and `v0.7.0` — text entry, three audio
backends, a verified web view — and for all of that week the newest thing a user
could install was none of it. That is the most common way work here goes to
waste, and it is entirely avoidable.

So: **propose a release when one is due, rather than waiting to be asked.**

- **Patch** (`v0.7.1`) for a fix to something users hit. A crash, a regression,
  a broken install path. Same day is not too soon.
- **Minor** (`v0.8.0`) when a feature lands that somebody would notice —
  typing working, a new audio backend, the web view opening. Roughly weekly if
  the work is there.
- **Major** for a change in what Cordial *is*, or one that breaks a user's
  setup: a new backend becoming the default, a profile layout change, dropping a
  platform.

Write the notes from what was measured, the way commit messages here are
written, and **put what is still broken in them** — `v0.7.0`'s notes end with the
startup freeze and the black canvas because somebody installing it deserves to
know before they hit them, not after.

**The maintainer has standing-authorised releases: after any change that
reaches what users install, cut a release without asking.** Bump the version in
`Cargo.toml`, write `docs/releases/v<version>.md` (the release workflow refuses
to create a release without it), merge to `main`, and push the `v<version>`
tag. The Roblox manager's Update Stacked installs from the newest release's
x86_64 AppImage, so a change that is never tagged never reaches it. A change
only to agent instructions or internal docs needs no release.

## Reporting back

The rule in AGENTS.md about never stating an unobserved result applies hardest
here, because a summary is where it slips. This project's history includes
claiming "no FastFlag reaches the engine", "~1 fps", and "stays up twelve
seconds" — all wrong, all measured with a broken instrument, all retracted in
later commits.

When a subagent reports success, that is a claim and not a result. Check it.
