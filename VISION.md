# OpenYoke Vision

## The one-sentence vision
OpenYoke is the thinking space for local AI: a private, local-first desktop app where every conversation is a branching graph you can explore, not a linear log — local models by default, with your own cloud API keys when you want a frontier model.

## Who it's for
- Privacy-sensitive professionals (legal, medical, security, enterprise) who cannot paste work into a hosted chatbot.
- Researchers and engineers who explore several competing directions at once.
- Local-LLM users who want an Ollama GUI without touching a terminal.
- People who already pay for Claude, GPT, or Gemini and want to use their own key  in a client they control, without a second subscription in between.

## Principles (these decide our tradeoffs)
1. **Local-first by default.** Nothing leaves the machine unless the user opts in. Cloud keys and web search are opt-in, never on by default in a privacy mode.
2. **Bring your own key, and know what that costs you.** Cloud models are used through the user's own API key, held locally and sent only to that provider — we are never a middleman and never proxy traffic through our own servers. The tradeoff is stated plainly rather than buried: when you pick a cloud model, that conversation's prompt and context are transferred to that provider and are subject to *their* terms and retention, not ours. Local models stay local; the UI makes it obvious which one you're talking to before you send.
3. **The graph is the product.** Features must strengthen branching, context isolation, and navigation. A better linear chat box is not our goal.
4. **Context isolation is a guarantee, not a UI trick.** Enforced in the Rust
   backend and covered by tests.
5. **Native and light.** Tauri 2 + Rust. No Electron, no bundled Chromium,
   no telemetry, ever.
6. **Your data is plain files.** JSON in a folder you chose. Always portable.

## Non-goals (for now)
- Multi-user server deployments / hosted SaaS.
- Reselling model access. We will not proxy cloud calls, resell credits, or issue our own keys — it's your key, your account, your bill.
- Being a general agent framework or workflow automation tool.
- Training or fine-tuning models.
- Mobile apps.

## What success looks like in 12 months
- One-click installers for macOS, Windows, Linux on every release.
- 10+ recurring contributors, <7-day median issue triage.
- "Branching conversations" is the feature people cite when they mention OpenYoke.