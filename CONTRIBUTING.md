# Contributing to OpenYoke

Thanks for your interest in OpenYoke! Whether it's a bug report, a feature idea, or a pull request, contributions are genuinely appreciated.

## The one rule: every change starts as an issue

> **No issue → no branch. No branch → no PR.**

This is not a formality, and it is not enforced by trust: a pull request that
doesn't close an issue, or that comes from a branch not named after that issue,
**fails CI and cannot be merged**. See
[`.github/workflows/pr-process.yml`](.github/workflows/pr-process.yml).

Nothing is exempt — a one-character typo fix gets an issue too. It costs you
thirty seconds, and in exchange every line in `git log` traces back to the
reason someone wrote it, every change is visible *before* the work starts
rather than after, and no one ever spends a weekend on something we'd have told
them we didn't want.

### The loop

**1 · Open an issue.** Pick the form that fits — [bug](https://github.com/saicv8/OpenYoke/issues/new?template=01-bug-report.yml),
[user story](https://github.com/saicv8/OpenYoke/issues/new?template=02-user-story.yml),
[enhancement](https://github.com/saicv8/OpenYoke/issues/new?template=03-enhancement.yml),
[task](https://github.com/saicv8/OpenYoke/issues/new?template=04-task.yml),
[epic](https://github.com/saicv8/OpenYoke/issues/new?template=05-epic.yml).
Blank issues are disabled on purpose: the forms are what get `type:` and
`area:` onto the issue for you.

**2 · Let it be triaged.** A maintainer adds `priority:` and `size:` and drops
`needs: triage`. Don't start coding on anything still labelled `needs: design`,
`needs: decision`, `needs: repro`, or `blocked` — those mean the *what* isn't
settled yet, and code written against an unsettled what gets thrown away.

**3 · Claim it.** Comment "I'd like to take this." You'll get `claimed` and the
assignment. (No need to ask on anything labelled `good first issue` — just say
you're starting.)

**4 · Create the branch _from the issue_.** On the issue page, right sidebar →
**Development** → **Create a branch**. GitHub names it for you and links branch,
issue, and later the PR together. That link is the whole point of doing it from
here rather than by hand.

Branching locally instead? Then match the same grammar yourself:

```bash
git switch main && git pull
git switch -c 42-graph-crashes-on-empty-tree     # <issue-number>-<slug>
git switch -c fix/42-graph-crashes-on-empty-tree # <type>/<issue-number>-<slug>, also fine
```

Anything else — `my-fix`, `saicharan/patch-1`, `dev` — is rejected by CI.

**5 · Do the work.** Keep it to the one issue; if you discover a second problem,
open a second issue. Prefix your commits with the issue number so the history
reads well after a squash merge:

```
[#42] guard the walk against an empty root
```

**6 · Open the PR.** The description **must** contain a closing keyword and the
number: `Closes #42` (`Fixes #42` / `Resolves #42` work too). Tick the
acceptance criteria you copied from the issue. The PR template already has the
line — fill in the number.

**7 · Merge.** Squash merge. The issue closes itself.

### Why the branch name matters too

Requiring only `Closes #42` in the PR body would let someone build first and
retro-fit an issue afterwards, which is the exact habit this is meant to
replace. Requiring the branch to carry the issue number means the issue existed
before the first commit did — and CI checks that the two numbers agree.

### Maintainers

The `issue-first` check must be a **required status check** on `main` for any
of this to bite:

```bash
gh api -X PUT repos/saicv8/OpenYoke/branches/main/protection \
  -H "Accept: application/vnd.github+json" \
  -f 'required_status_checks[strict]=true' \
  -f 'required_status_checks[contexts][]=issue-first' \
  -F 'enforce_admins=false' -F 'required_pull_request_reviews=null' -F 'restrictions=null'
```

`enforce_admins=false` leaves exactly one escape hatch, for a P0 that is
actively losing user data. Using it means opening the issue immediately
afterwards and linking the commit to it.

## Ways to help

- 🐛 **Report bugs** — open an issue with steps to reproduce, your OS, and what you expected.
- 💡 **Suggest features** — open an issue describing the problem you're trying to solve.
- 📖 **Improve docs** — typos, clarifications, and better examples are all welcome.
- 🔧 **Send a pull request** — from a branch created off the issue it closes. See [the loop](#the-loop).

## Development setup

```bash
# Prerequisites: Rust (stable), Node.js, and Ollama running locally.
npm install
npx tauri dev
```

Backend logic is unit-tested in Rust:

```bash
cd src-tauri
cargo test
```

## Project layout

- `static/` — the web UI (vanilla HTML/CSS/JS, no bundler).
- `src-tauri/src/` — the Rust backend:
  - `tree.rs` — pure branching-tree logic + the context-isolation walk (well covered by tests).
  - `ollama.rs` — the single seam to the Ollama API.
  - `storage.rs` — the user's data folder.
  - `catalog.rs` — the model library.
  - `main.rs` — Tauri commands and persistence.

## 🙋 Want to contribute?

| I want to… | Go here |
|---|---|
| Pick up a beginner-friendly task | [good first issues](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22+no%3Aassignee) |
| Find any unclaimed work | [help wanted, unassigned](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22+no%3Aassignee+-label%3Aclaimed) |
| Work on the branching canvas | [area: graph](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22area%3A+graph%22) |
| Do something small (under a day) | [size: XS + S](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22size%3A+XS%22%2C%22size%3A+S%22) |
| Improve Rust/Tauri packaging | [area: packaging](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22area%3A+packaging%22) |
| See what's broken | [open bugs by priority](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22type%3A+bug%22+sort%3Areactions-%2B1-desc) |
| See where the project is heading | [ROADMAP.md](ROADMAP.md) · [VISION.md](VISION.md) |

## How we label issues

Every triaged issue carries exactly four labels — one from each axis:

    type: <bug|feature|enhancement|task|docs|epic>   what kind of work
    area: <graph|ollama|cloud-providers|...|docs>    where in the product
    priority: <P0|P1|P2|P3>                          how urgent (maintainer)
    size: <XS|S|M|L|XL>                              rough effort (maintainer)

Plus, when relevant:
  needs: *   → blocked on something from you or from us. Read the comment.
  blocked    → waiting on another issue or an upstream project
  claimed    → someone's on it. Auto-removed after 14 days of silence.

**You only ever need to set `type:` and `area:`** — the issue form does it
for you. Priority and size are ours to set, so please don't relabel them.

### Claiming an issue
Comment "I'd like to take this" — see [step 3 of the loop](#the-loop).

### Our promise
`good first issue` means a maintainer has personally verified the fix path
and written implementation pointers into the issue. If one of them turns out
to be a rabbit hole, say so — that's a bug in our labelling, not in you.

## Pull request guidelines

1. It closes an issue, and it comes from that issue's branch — [the one rule](#the-one-rule-every-change-starts-as-an-issue). CI checks both.
2. Keep changes focused; one issue per PR.
3. If you touch backend logic, add or update tests (`cargo test` should pass).
4. Match the existing code style — small, pure, testable helpers on the Rust side; plain `invoke()` calls on the JS side.
5. Write a clear PR description explaining the *why*, not just the *what*. The issue holds the *what*.

## Code of conduct

Be kind and constructive. We want OpenYoke to be a welcoming project for everyone.
