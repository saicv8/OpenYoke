# ROADMAP

Where OpenYoke is going and why. [VISION.md](VISION.md) says what we're
building and what we refuse to build; this file says **in what order**.

## How to read this

- Work is grouped into **milestones**. Each milestone holds a few **epics**; each epic becomes a `type: epic` issue whose child stories are the actual work. Epics are never implemented directly.
- Every epic names the **VISION principle** it serves and its primary **`area:` label**, so you can jump straight from a line here to the issues.
- **No dates.** A milestone ships when its epics close. Horizons are ordered (*Now → Next → Later*), not scheduled.
- Anything on this roadmap is at least `priority: P2` ("a committed roadmap item"). Priority and size are maintainer-set — see [CONTRIBUTING.md](CONTRIBUTING.md).

Status: ✅ shipped · 🚧 in progress · 📋 planned · 💡 candidate

---

## Shipped — v0.1.0

The base the rest of this builds on:

- ✅ Branching conversation graph — free-drag canvas, pan/zoom, saved node positions, branch from any node.
- ✅ Context isolation enforced in `src-tauri/src/tree.rs`, not in the UI.
- ✅ Streaming responses over Tauri channels (`sse.rs`).
- ✅ In-app Ollama model management — live catalog, pull with progress, delete.
- ✅ Bring-your-own-key cloud models: Anthropic, OpenAI-compatible (OpenRouter, Groq, …), Google Gemini.
- ✅ Web search for any model via DuckDuckGo + URL retrieval, with citations.
- ✅ Plain-JSON storage in a folder you choose; light/dark/system themes, text size, `⌘B` sidebar.
- ✅ 96 Rust unit tests covering the tree walk, providers, search and storage
  (plus 5 network-gated tests behind `#[ignore]`).

---

## Now — v0.2.0 · "A graph you can live in"

Today the graph is pleasant at 10 nodes and awkward at 100. This milestone is about living in it for real work.

### Epic: Control over a running generation
**Outcome:** no generation you can't stop, and no answer you can't redo.
*Principle 3 — the graph is the product.* · `area: streaming`

- 📋 **Stop** button that cancels an in-flight stream and keeps partial output.
- 📋 Regenerate an answer — as a sibling node, so the original survives.
- 📋 Edit a question and re-ask it as a new branch from the same parent.
- 📋 Per-node error state with a one-click retry (network, bad key, model gone).

### Epic: Navigating a large tree
**Outcome:** a 100-node conversation is still readable.
*Principle 3 — the graph is the product.* · `area: graph`

- 📋 Collapse and expand a subtree.
- 📋 "Tidy" auto-layout and fit-to-view.
- 📋 Search within a conversation and jump to matching nodes.
- 📋 Node labels that show more than the first few words (auto-title a node).
- 📋 Breadcrumb of the active path in the side panel.

### Epic: Keyboard and accessibility
**Outcome:** the whole app is usable without a mouse.
*Principle 5 — native and light.* · `area: ui`

- 📋 Keyboard navigation between nodes; ask, branch, delete via shortcuts.
- 📋 Visible focus states and a shortcut cheat-sheet dialog.
- 📋 Screen-reader labels for canvas nodes and streaming regions.
- 📋 Contrast audit of both themes.

**Out of scope for v0.2.0:** new providers, tool calling, anything hosted.

---

## Next — v0.3.0 · "Installs you can trust"

VISION's 12-month bar is one-click installers on every release. Right now the only CI is issue triage and label sync — nothing builds or tests a PR.

### Epic: Continuous integration
**Outcome:** every PR is built and tested on all three platforms before merge.
*Principle 4 — guarantees are covered by tests.* · `area: packaging`

- 📋 `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check` on every PR.
- 📋 Build the Tauri bundle on macOS, Windows and Linux.
- 📋 Cached toolchains so the run stays under a few minutes.

### Epic: Signed releases
**Outcome:** a tag produces installers a stranger can safely run.
*Principle 5 — native and light.* · `area: packaging`

- 📋 Tag-triggered release: `.dmg`, `.msi`, `AppImage` + `.deb`.
- 📋 macOS code signing and notarization; Windows signing.
- 📋 Generated release notes and checksums.

### Epic: Updates and first run
**Outcome:** installing and updating never sends you to a terminal.
*Principle 1 — local-first by default.* · `area: packaging` · `area: ollama`

- 📋 Opt-in update check via the Tauri updater — no telemetry, ever.
- 📋 Show the version in **Settings → About**, so bug reports can cite it.
- 📋 First-run detects whether Ollama is installed and running, and guides you.
- 📋 Suggest and pull a starter model sized to the machine.
- 💡 Optionally bundle the Ollama runtime so there's no separate install.

**Out of scope for v0.3.0:** auto-installing Ollama silently; any phone-home
beyond the explicit update check.

---

## Later — v0.4.0 · "Every model behind one seam"

Four provider modules (`ollama.rs`, `anthropic.rs`, `openai.rs`, `google.rs`)
currently repeat their own streaming and error handling. Collapsing them is
what makes per-branch model comparison cheap.

### Epic: Provider abstraction
**Outcome:** adding a provider is one file and a conformance test.
*Principle 2 — bring your own key.* · `area: cloud-providers`

- 📋 One trait for list / chat-stream / cancel, with a shared error contract.
- 📋 A conformance test suite every provider must pass.
- 📋 Document "how to add a provider" in CONTRIBUTING.

### Epic: Compare models across branches
**Outcome:** ask one question of three models and read the answers side by side.
*Principle 3 — the graph is the product.* · `area: graph`

- 📋 Fan a question out to several models as sibling branches in one action.
- 📋 Side-by-side branch comparison view.
- 📋 Show which model produced each node, on the node itself.

### Epic: Know what a cloud call costs
**Outcome:** principle 2's promise is visible, not just written down.
*Principle 2 — know what that costs you.* · `area: cloud-providers`

- 📋 Token usage per node and per conversation.
- 📋 Estimated cost for cloud models; "local — free" for Ollama.
- 📋 A clear indicator, before you send, of where this prompt is going.

### Epic: Key handling hardening
**Outcome:** a leaked settings file is not a leaked API key.
*Principle 1 — local-first by default.* · `area: settings`

- 📋 Store cloud keys in the OS keychain instead of `settings.json`.
- 📋 Migrate existing keys on upgrade.
- 📋 Redact keys from every error message, log line and export.

---

## Toward v1.0.0

1.0 is a quality bar, not a feature list. We ship it when:

- Signed installers for macOS, Windows and Linux ship on every release.
- The storage format is versioned, with a tested migration path.
- The provider seam is stable and documented.
- No open `priority: P0` or `P1` issues; median issue triage under 7 days.
- Accessibility pass complete on the graph and all dialogs.
- 10+ recurring contributors.

---

## Candidates — not scheduled

Good ideas that need a champion, a design, or a `needs: decision` call:

- 💡 Tool / function calling, possibly via MCP.
- 💡 Local document context (RAG) with local embeddings — no cloud indexing.
- 💡 Attachments: images and files as node input.
- 💡 A prompt library — reusable system prompts, per conversation.
- 💡 Alternate search backends (SearXNG, a self-hosted instance, BYO key).
- 💡 Summarize a long path to compress context before it overflows.
- 💡 Encryption at rest for the storage folder.
- 💡 Merge or diff two branches.
- 💡 Localization.

Want one of these? Open a `type: feature` issue arguing the case against
VISION.md's principles — that's how something moves from here to a milestone.

## Not planned

Straight from [VISION.md](VISION.md)'s non-goals; issues asking for these get
closed `wontfix` with a link:

- Hosted SaaS or multi-user server deployments.
- Proxying or reselling model access — it's your key, your account, your bill.
- A general agent framework or workflow automation tool.
- Training or fine-tuning models.
- Mobile apps.
- Telemetry or analytics of any kind.

---

## Helping

Pick anything marked 📋 — or find work by label:

| I want to… | Go here |
|---|---|
| Start small | [good first issues](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22+no%3Aassignee) |
| Take unclaimed work | [help wanted, unassigned](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22+no%3Aassignee+-label%3Aclaimed) |
| See the current milestone | [milestones](https://github.com/saicv8/OpenYoke/milestones) |
| See the epics | [type: epic](https://github.com/saicv8/OpenYoke/issues?q=is%3Aissue+is%3Aopen+label%3A%22type%3A+epic%22) |

This roadmap is a direction, not a delivery commitment. It changes by pull
request against this file — if you think the order is wrong, say so there.
