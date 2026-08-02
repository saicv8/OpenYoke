# Contributing to OpenYoke

Thanks for your interest in OpenYoke! Whether it's a bug report, a feature idea, or a pull request, contributions are genuinely appreciated.

## Ways to help

- 🐛 **Report bugs** — open an issue with steps to reproduce, your OS, and what you expected.
- 💡 **Suggest features** — open an issue describing the problem you're trying to solve.
- 📖 **Improve docs** — typos, clarifications, and better examples are all welcome.
- 🔧 **Send a pull request** — see below.

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
Comment "I'd like to take this." We'll add `claimed` and assign you.
No need to ask permission on anything labelled `good first issue`.

### Our promise
`good first issue` means a maintainer has personally verified the fix path
and written implementation pointers into the issue. If one of them turns out
to be a rabbit hole, say so — that's a bug in our labelling, not in you.

## Pull request guidelines

1. Fork the repo and create a branch from `main`.
2. Keep changes focused; one topic per PR.
3. If you touch backend logic, add or update tests (`cargo test` should pass).
4. Match the existing code style — small, pure, testable helpers on the Rust side; plain `invoke()` calls on the JS side.
5. Write a clear PR description explaining the *why*, not just the *what*.

## Code of conduct

Be kind and constructive. We want OpenYoke to be a welcoming project for everyone.
