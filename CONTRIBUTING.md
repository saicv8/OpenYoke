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

## Pull request guidelines

1. Fork the repo and create a branch from `main`.
2. Keep changes focused; one topic per PR.
3. If you touch backend logic, add or update tests (`cargo test` should pass).
4. Match the existing code style — small, pure, testable helpers on the Rust side; plain `invoke()` calls on the JS side.
5. Write a clear PR description explaining the *why*, not just the *what*.

## Code of conduct

Be kind and constructive. We want OpenYoke to be a welcoming project for everyone.
