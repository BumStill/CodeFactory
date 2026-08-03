<div align="center">

<img src="docs/icon.png" alt="CodeFactory" width="128" />

# CodeFactory

**Local-first AI coding assistant for Windows & macOS — bring a folder, set a goal, ship faster.**

[![Latest Release](https://img.shields.io/github/v/release/BumStill/CodeFactory)](https://github.com/BumStill/CodeFactory/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/BumStill/CodeFactory/actions/workflows/ci.yml/badge.svg)](https://github.com/BumStill/CodeFactory/actions)

[Download](https://github.com/BumStill/CodeFactory/releases/latest) · [Issues](https://github.com/BumStill/CodeFactory/issues)

</div>

<!-- README-CONTRACT: evergreen
Release-specific versions and change details belong in GitHub Release notes.
Update this file in the same PR when a public product, install, privacy, or
platform promise changes; keep download links version-neutral.
-->

---

CodeFactory is a Tauri desktop app that turns ideas into shipped code. It talks
to any OpenAI-compatible model provider — OpenRouter, DeepSeek, Anthropic,
OpenAI, local LMStudio / Ollama — and gives the model real tools to read,
write, edit, search, and run code on your machine, with the safety rails you'd
want on something doing that.

## Why

Most AI coding tools are chat windows that paste snippets. The interesting
work — running tests, applying multi-file edits, recovering from a flubbed
refactor, knowing when to stop and ask — happens between the snippets.
CodeFactory is built around that loop.

It runs locally — your conversations and settings stay on your machine
(`%APPDATA%\com.codefactory.app\` on Windows, `~/Library/Application
Support/com.codefactory.app/` on macOS) — and there's no telemetry. Bring
your own API key, or sign in with ChatGPT for the Codex-backed provider.

## Features

| | |
|---|---|
| 🔌 **Multi-provider** | OpenRouter, DeepSeek, Anthropic, OpenAI, LMStudio, Ollama. Per-endpoint custom models + auto vendor-prefix normalization (`deepseek/v4-pro` → `v4-pro` for direct providers). |
| 🛠 **Real tool use** | `read_file`, `write_file`, `edit_file`, `bash` (PowerShell), `grep`, `glob`. Per-file locks + atomic writes + post-write byte-level integrity check. |
| 📄 **Documents** | Generate and edit Office files from chat: PowerPoint (`read`/`edit`/`format`/`write_pptx`), Word (`write_docx`), Excel (`read_xlsx` / `edit_xlsx` — e.g. summarize a column into the next one row by row). Drop a `.pptx` / `.docx` / `.pdf` / `.xlsx` into the message box and the agent reads it. |
| 🗂 **Sessions** | Lightweight quick chats and full project workspaces share one rail — rename or delete inline, and multiple sessions stream concurrently without blocking each other. |
| 🧠 **Context-aware** | Live context-window meter, adaptive compression of oversized tool results when nearing the model's limit, `reasoning_content` propagation for DeepSeek reasoner / Claude thinking-mode models. |
| 🛡 **Safe by default** | Configurable permission policy per-tool. Path-typo detection blocks hallucinated targets like `app/__iniy/` before they write. Daily DB backups with 7-day rolling retention. |
| 🤖 **Subagents** | Conversation-native task delegation → parallel subagent dispatch, shared brief, verification engine, evidence pack auto-collection. Long-lived specs stay in the repository and travel with Git. |
| 🪝 **Hooks & Skills** | Run scripts on tool events (commit-on-edit, lint-on-write). Create, edit, import, or let the agent search for skill packs (system prompts + slash commands) right from the chat box — pull in Superpowers / OpenSpec-style skills with a preview-then-enable step. |
| 🌐 **MCP client** | Connect Model Context Protocol servers for arbitrary tool extension. |
| 🌐 **On-demand browser** | Task-scoped managed browser sessions open an embedded pane only when needed; pause for login or approval, then recover the session without leaving an idle browser daemon behind. |
| 📦 **Docker sandbox** | Optional `sandbox_mode: Docker` runs every shell/tool command in a disposable container instead of your host shell. |
| 🔔 **IM notifications** | Optional WeCom / Feishu / generic-JSON webhook pings you when a task finishes, fails, or needs permission — fire-and-forget, no secrets in the payload. |
| 🚚 **Controlled delivery** | Turn a completed task into an auditable PR → CI → merge → release flow. Recoverable CI/metadata/branch failures return to a bounded repair loop; genuine permission or policy blockers stay explicit. |
| 🔁 **Auto-update** | Signed updates over GitHub Releases. New version arrives → in-app banner → one-click install + relaunch. Your data stays. |

## Install

Grab the latest build from
[Releases](https://github.com/BumStill/CodeFactory/releases/latest):

- **Windows** — `CodeFactory_X.Y.Z_x64-setup.exe` (NSIS installer, no admin
  rights required)
- **macOS (Apple Silicon)** — `CodeFactory_X.Y.Z_aarch64.dmg` (macOS 11+)

> **Windows SmartScreen warning is expected on first run.** CodeFactory
> isn't currently signed with a paid Authenticode certificate (working on
> it — see [#code-signing](#code-signing-status)), so SmartScreen treats
> it as an unrecognised app. To install:
>
> 1. Click **More info** on the blue SmartScreen popup
> 2. Click **Run anyway**
>
> If you'd like extra confidence before doing that, you can verify the
> installer's integrity:
>
> ```pwsh
> # The expected SHA-256 is published in each release's asset list:
> # https://github.com/BumStill/CodeFactory/releases/latest
> Get-FileHash CodeFactory_*_x64-setup.exe
> ```
>
> Updates installed via the in-app updater bypass this dialog because
> the updater verifies the Tauri signature (`.sig`) against the embedded
> public key — only the first manual install gets the warning.

> **macOS Gatekeeper warning is expected on first launch.** CodeFactory isn't
> notarized with an Apple Developer ID yet, so macOS refuses to open it
> directly. To get past it:
>
> 1. Open the `.dmg` and drag **CodeFactory** to **Applications**
> 2. In Applications, **right-click (or Control-click) CodeFactory → Open**
> 3. Click **Open** in the dialog (only needed the first time)
>
> Alternatively, after the blocked first launch, allow it under **System
> Settings → Privacy & Security → Open Anyway**. As on Windows, in-app updates
> afterward are verified by the Tauri signature and don't prompt.

On first launch you'll be prompted for an API key. Anything OpenAI-compatible
works — OpenRouter is the easiest if you want access to multiple frontier
models with one key.

### Code-signing status

Both platforms currently ship **unsigned** binaries with Tauri-signed updates;
the first-install OS warning is expected and one-time. The plan:

- **Now**: unsigned NSIS installer (Windows) and unsigned `.dmg` (macOS) +
  Tauri-signed auto-updates — good enough for technical users who can dismiss
  the SmartScreen / Gatekeeper prompt once
- **Next**: apply for [SignPath.io](https://signpath.io/) free OSS code signing
  on Windows (eliminates the SmartScreen warning at no cost)
- **Later**: Apple Developer ID notarization for macOS, and a full EV cert on
  Windows, if download volume justifies the spend

## Quick start

1. Open a project folder (sidebar → `+` → pick a directory).
2. Pick a model in the top-right model picker.
3. Try one of the welcome prompts, or write your own.

The agent reads, writes, and runs commands inside your project's directory.
Tool calls that touch files or shell ask for permission the first time per
session (configurable in Settings → Permissions).

## Build from source

```pwsh
git clone https://github.com/BumStill/CodeFactory
cd CodeFactory
pnpm install
pnpm tauri dev
```

Requires Rust stable + Node 20 + pnpm 10. The first build is ~3 min as it
compiles the Tauri toolchain; subsequent builds are seconds.

To produce installers:

```pwsh
pnpm tauri build
```

On Windows the NSIS installer lands in
`src-tauri/target/release/bundle/nsis/`; on Apple Silicon macOS the DMG lands
in `src-tauri/target/release/bundle/dmg/`.

For a deeper walkthrough of the codebase layout, test commands, and
where runtime data lives, see [DEVELOPMENT.md](DEVELOPMENT.md).
Release / versioning policy lives in [VERSIONING.md](VERSIONING.md).

## Data & privacy

Everything lives on your machine:

| What | Where |
|---|---|
| Sessions, messages, costs (SQLite) | `%APPDATA%\com.codefactory.app\codefactory.db` (Windows) / `~/Library/Application Support/com.codefactory.app/codefactory.db` (macOS) |
| Settings (endpoints, permissions, hooks) | same app-data folder, `settings.json` |
| API keys | OS keychain — Windows Credential Manager / macOS Keychain |
| Daily DB backups (7 day retention) | same app-data folder, `codefactory.db.backup-YYYYMMDD` |

Uninstall preserves all of the above. Reinstall picks up where you left off.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  React + Vite + Tailwind  ◄──┐                              │
│  ChatPage · MessageList · ToolCallCard · ContextUsageBar    │
│                              │ Tauri IPC                    │
│  Rust (tokio + axum-less)    ▼                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Agent loop                                           │   │
│  │   ├─ SSE-buffered streaming (OpenAI / Anthropic)     │   │
│  │   ├─ Tool dispatcher (bash, read, write, edit,       │   │
│  │   │   grep, glob, MCP) with per-file locks           │   │
│  │   ├─ Context manager (compression + reasoning)       │   │
│  │   └─ Hook runner (pre / post tool)                   │   │
│  └──────────────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Storage: SQLite via sqlx, atomic writes, daily backup│   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

Backend lives in [src-tauri/src/](src-tauri/src/), frontend in
[src/](src/). Conventional-commit messages drive
auto-generated release notes via [`cliff.toml`](cliff.toml).

## Memory & self-evolution

CodeFactory's tagline is **软件工厂 · 本地助手 · 自进化** — it's meant to get
better from its own use. Concretely, today: sessions are mined locally for
reusable patterns with no model call and no data leaving your machine, and
accepted lessons land in a per-project `memory.md` that's injected back into
future context — automatically, no "remember" button required. Skill
proposals and tool-permission tightening go through a human review step
before they take effect. This is still an early, honestly-scoped version of
the idea — memory has no decay/retirement loop yet, and full autonomous
self-modification is not implemented. See
[docs/self-evolution/README.md](docs/self-evolution/README.md) for the
current state and phased roadmap, and [docs/BACKLOG.md](docs/BACKLOG.md) for
what's planned next.

## Roadmap

- [x] Per-endpoint active model
- [x] Auto-update over GitHub Releases
- [x] DeepSeek `reasoning_content` round-tripping
- [x] Path-typo detection harness
- [x] Export / import settings + DB
- [x] Office document generation (PowerPoint / Word / Excel)
- [x] Skill management from chat (create / edit / import / search)
- [x] macOS (Apple Silicon) build
- [ ] Welcome page with template projects
- [ ] Linux build

## License

Apache-2.0 — see [LICENSE](LICENSE).
