<div align="center">

# OpenYoke

### A private, local-first desktop app for chatting with open-source AI models — with branching conversations you explore as a visual graph.

OpenYoke is an open-source **Ollama desktop GUI** and **private ChatGPT alternative** that runs entirely on your machine. No cloud, no accounts, no data leaving your computer. Manage models, chat with streaming responses, and branch any conversation into a tree you can navigate like a canvas.

![License: MIT](https://img.shields.io/badge/License-MIT-14b8a6.svg)
![Platform](https://img.shields.io/badge/platform-macOS%20·%20Windows%20·%20Linux-4a5568)
![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24c8db)
![Backend: Rust](https://img.shields.io/badge/backend-Rust-dea584)
![Models: Ollama](https://img.shields.io/badge/models-Ollama-000000)
![PRs Welcome](https://img.shields.io/badge/PRs-welcome-14b8a6.svg)

</div>

<!--
  📸 Add a hero screenshot here for maximum impact, e.g.:
  ![OpenYoke branching conversation graph](docs/screenshot.png)
  A screenshot of the graph view is the single biggest thing you can add to earn stars.
-->

---

## Why OpenYoke?

Most local-LLM tools give you a plain chat box. OpenYoke gives you a **thinking space**:

- 🌳 **Branching conversations, not a linear log.** Every exchange is a node in a tree. Ask a follow-up, or branch off in a new direction from *any* earlier point — then see the whole shape of your thinking as an interactive, n8n-style graph.
- 🧠 **Context stays on the path.** When you branch, the model only sees the messages along that branch — sibling branches never bleed into each other. Explore competing ideas in parallel without cross-contamination.
- 🔒 **100% local and private.** Your conversations and model weights live on your machine. Nothing is sent anywhere. Perfect for sensitive work and offline use.
- 🖥️ **No terminal required.** Browse a curated model library, download models with a live progress bar, and delete them — all from inside the app.
- ⚡ **Fast and lightweight.** A native [Tauri](https://tauri.app) shell with a Rust backend. No Electron bloat, no bundled Chromium.

## Features

- 🗂️ **Branching conversation graph** — a free-drag canvas of nodes (each node is one question + answer). Pan, zoom, drag to arrange; branch from any node.
- ✍️ **Streaming responses** — tokens render live as the model generates, like the tools you already love.
- 📚 **In-app model management** — browse open models (Llama, Qwen, Mistral, Gemma, Phi, DeepSeek, and more), pull them with progress, and remove them to reclaim disk.
- 💾 **Your data, your folder** — pick where conversations and settings are saved; everything persists across restarts as plain JSON.
- 🎨 **Minimal, Notion-inspired UI** — clean, compact, and out of your way.
- 🧩 **Provider-ready architecture** — Ollama today, with a clean seam for embedded or cloud backends next.

## How branching works

A conversation is a **tree**. Each node holds one interaction — your question and the model's answer.

```
            ┌─ "Which frontend framework?" ─┐
"Build a    │                               ├─ "How do I deploy React?"
 web app?" ─┤                               └─ "Compare React vs Svelte"
            └─ "What about the backend?" ──── "Show me an Express example"
```

Ask a follow-up to extend a branch; branch again from any node to explore an alternative. When the model answers at a node, it sees **only the path from the root to that node** — so your alternate branches stay independent. This isolation is enforced in the Rust backend, not just the UI.

## Getting started

### Prerequisites

- [Rust toolchain](https://www.rust-lang.org/tools/install) (stable)
- [Node.js](https://nodejs.org) (for the Tauri CLI)
- [Ollama](https://ollama.com) running locally (install below)

### Install Ollama

Install once; after that you manage models entirely from OpenYoke's **Models** tab.

```bash
# macOS
brew install ollama                                   # or the .dmg from ollama.com/download

# Windows
winget install Ollama.Ollama                          # or the installer from ollama.com/download

# Linux
curl -fsSL https://ollama.com/install.sh | sh
```

Make sure it's running (`ollama serve`) — it listens on `http://127.0.0.1:11434`, the address OpenYoke connects to by default. You do **not** need to pull models from the terminal; do it from the Models tab.

### Run the app

```bash
npm install
npx tauri dev
```

### Build a desktop bundle

```bash
npx tauri build
```

### Run the tests

```bash
cd src-tauri
cargo test
```

## Data & privacy

On first launch OpenYoke asks you to choose a **storage folder**. Your conversations (full history and branch structure) and settings are saved there as JSON, so nothing is lost between sessions.

- **Your data**: `conversations.json` and `settings.json` in the folder you pick.
- **A tiny pointer**: a `config.json` in the OS app-data dir remembers *where* your folder is — the only thing stored outside it.
- **Model weights** are managed by Ollama in `~/.ollama/`, not by OpenYoke.

Nothing is transmitted off your machine. OpenYoke only talks to your local Ollama instance.

## Architecture

OpenYoke is a **pure Tauri** app — a Rust backend and a dependency-free web UI in one native binary. The frontend calls the backend over Tauri's `invoke` bridge; the backend talks to Ollama over HTTP.

```
┌──────────────────────────────────────┐
│  OpenYoke (native desktop app)        │
│                                       │
│  WebView UI (static/)                 │
│      │  invoke() / Channel            │
│      ▼                                │
│  Rust backend (src-tauri/)            │──HTTP──▶  Ollama (:11434)
│   • tree.rs   branching + context     │
│   • ollama.rs model + streaming chat  │
│   • storage.rs your data folder       │
│   • catalog.rs model library          │
└──────────────────────────────────────┘
```

- `static/` — the graph + model-management UI (vanilla HTML/CSS/JS, no bundler).
- `src-tauri/src/tree.rs` — pure branching-tree logic and the context-isolation walk.
- `src-tauri/src/ollama.rs` — the single seam to Ollama (`list_models`, `chat_stream`, `pull_model`, `delete_model`).
- `src-tauri/src/storage.rs` — resolves the user-chosen storage folder.
- `src-tauri/src/catalog.rs` — the browsable model library (remote-refreshable, bundled fallback).

## Roadmap

- [ ] Provider abstraction for multiple backends (the `ollama` module is the seam)
- [ ] Optionally bundle Ollama so no separate install is needed
- [ ] Richer tool / function-calling integration
- [ ] Multi-model agent mode (different models per branch)
- [ ] Export a branch or the whole tree (Markdown / JSON)

## Contributing

Contributions are welcome — issues, ideas, and pull requests all help. See [CONTRIBUTING.md](CONTRIBUTING.md) to get started.

## License

[MIT](LICENSE) © OpenYoke contributors.

---

<div align="center">

**Keywords:** local-first AI · Ollama GUI · private ChatGPT alternative · offline LLM · open-source models · branching conversations · conversation tree · tree of thought · llama · mistral · qwen · Tauri · Rust · desktop AI app

If OpenYoke is useful to you, consider giving it a ⭐ — it genuinely helps.

</div>
