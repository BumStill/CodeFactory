<div align="center">

<img src="docs/icon.png" alt="CodeFactory" width="128" />

# CodeFactory

**AI coding assistant for Windows — bring a folder, set a goal, ship faster.**

[![Latest Release](https://img.shields.io/github/v/release/BumStill/CodeFactory)](https://github.com/BumStill/CodeFactory/releases/latest)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/BumStill/CodeFactory/actions/workflows/ci.yml/badge.svg)](https://github.com/BumStill/CodeFactory/actions)

[Download](https://github.com/BumStill/CodeFactory/releases/latest) · [Issues](https://github.com/BumStill/CodeFactory/issues)

</div>

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

It runs locally, your conversations and settings live in
`%APPDATA%\com.codefactory.app\`, and there's no telemetry. Bring your own
API key.

## Features

| | |
|---|---|
| 🔌 **Multi-provider** | OpenRouter, DeepSeek, Anthropic, OpenAI, LMStudio, Ollama. Per-endpoint custom models + auto vendor-prefix normalization (`deepseek/v4-pro` → `v4-pro` for direct providers). |
| 🛠 **Real tool use** | `read_file`, `write_file`, `edit_file`, `bash` (PowerShell), `grep`, `glob`. Per-file locks + atomic writes + post-write byte-level integrity check. |
| 🧠 **Context-aware** | Live context-window meter, adaptive compression of oversized tool results when nearing the model's limit, `reasoning_content` propagation for DeepSeek reasoner / Claude thinking-mode models. |
| 🛡 **Safe by default** | Configurable permission policy per-tool. Path-typo detection blocks hallucinated targets like `app/__iniy/` before they write. Daily DB backups with 7-day rolling retention. |
| 🤖 **Subagents** | Spec → tasks → parallel subagent dispatch, shared brief, verification engine, evidence pack auto-collection. |
| 🪝 **Hooks & Skills** | Run scripts on tool events (commit-on-edit, lint-on-write). Drop-in skill packs with system prompts and slash commands. |
| 🌐 **MCP client** | Connect Model Context Protocol servers for arbitrary tool extension. |
| 🔁 **Auto-update** | Signed updates over GitHub Releases. New version arrives → in-app banner → one-click install + relaunch. Your data stays. |

## Install

Download `CodeFactory_X.Y.Z_x64-setup.exe` from
[Releases](https://github.com/BumStill/CodeFactory/releases/latest) and run it.
No admin rights required.

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

On first launch you'll be prompted for an API key. Anything OpenAI-compatible
works — OpenRouter is the easiest if you want access to multiple frontier
models with one key.

### Code-signing status

Authenticode signing is on the near-term roadmap. The plan:

- **Now**: unsigned NSIS installer + Tauri-signed updates (good enough for
  technical users who can dismiss SmartScreen once)
- **Next**: apply for [SignPath.io](https://signpath.io/) free OSS code
  signing (eliminates the SmartScreen warning at no cost)
- **Later**: full EV cert if download volume justifies the spend

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

To produce an installer:

```pwsh
pnpm tauri build
```

Output lands in `src-tauri/target/release/bundle/nsis/`.

## Data & privacy

Everything lives on your machine:

| What | Where |
|---|---|
| Sessions, messages, costs (SQLite) | `%APPDATA%\com.codefactory.app\codefactory.db` |
| Settings (endpoints, permissions, hooks) | `%APPDATA%\com.codefactory.app\settings.json` |
| API keys | Windows Credential Manager |
| Daily DB backups (7 day retention) | `%APPDATA%\com.codefactory.app\codefactory.db.backup-YYYYMMDD` |

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

## Roadmap

- [x] Per-endpoint active model
- [x] Auto-update over GitHub Releases
- [x] DeepSeek `reasoning_content` round-tripping
- [x] Path-typo detection harness
- [ ] Export / import settings + DB
- [ ] Welcome page with template projects
- [ ] macOS + Linux builds

## License

Apache-2.0 — see [LICENSE](LICENSE).
