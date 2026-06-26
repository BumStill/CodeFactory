# Development notes

Quick reference for picking the project up on a fresh machine.

## Prerequisites

- **Rust** stable (1.77+). [`rustup`](https://rustup.rs/) is the easiest way.
- **Node 20+** and **pnpm 10+**. (Corepack: `corepack enable pnpm`.)
- **Tauri build dependencies** for your OS — see
  https://tauri.app/start/prerequisites/. On Windows: Microsoft C++
  Build Tools + WebView2 (usually preinstalled on Windows 10/11).
- **Git CLI** on PATH. Used by `agent/checkpoint.rs` and `commands/git.rs`.

## First-time setup

```pwsh
git clone https://github.com/BumStill/CodeFactory
cd CodeFactory
pnpm install            # installs JS deps + Tauri CLI
cargo fetch --manifest-path src-tauri/Cargo.toml   # warm Rust deps
git config core.hooksPath .githooks                # enable local sync gates
```

## Git sync gate

CodeFactory often has multiple branches or agents moving at once. Before
committing, the versioned pre-commit hook fetches the default branch and blocks
the commit unless the current HEAD already contains latest `origin/main`.

If it blocks, sync first:

```pwsh
git fetch --prune origin main
git merge origin/main
```

Resolve conflicts, rerun the relevant tests, then commit. `CODEFACTORY_SKIP_SYNC_GATE=1`
is reserved for explicitly approved `hotfix bypass` work.

## Run dev

```pwsh
pnpm tauri dev          # starts Vite + Rust + opens the desktop window
```

Notes:
- First cold Rust build takes ~3 min; subsequent rebuilds are seconds.
- Tauri uses Vite at `http://127.0.0.1:1420`. The IPv4 host binding is
  important — WebView2 doesn't resolve `localhost` reliably on Windows
  (see `vite.config.ts`).
- Auto-update is **skipped in dev** (`UpdaterBanner` checks `import.meta.env.DEV`).
  To exercise it, build and install a release artifact.

## Tests

```pwsh
# Rust unit + integration tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend tests (Vitest + jsdom + RTL)
pnpm test
```

CI runs both on every push to `main`. See `.github/workflows/ci.yml`.

## Releasing

See [VERSIONING.md](VERSIONING.md). TL;DR:

```pwsh
.\scripts\bump-version.ps1 [patch|minor|major]
```

This bumps the three version files, commits, tags `vX.Y.Z`, and pushes.
The release workflow builds the Windows installer, signs it with the
Tauri updater key (stored in GitHub Secrets — `TAURI_SIGNING_PRIVATE_KEY`
and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), and publishes the GitHub
Release.

## Where things live

| Path | What |
|---|---|
| `src/` | React + TypeScript frontend. |
| `src/components/` | Reusable UI. Notable: `useStickyAutoScroll.ts` + `.test.tsx` (chat scroll state machine), `CheckpointsPanel.tsx` (git rollback), `RememberButton.tsx` (`.codefactory/memory.md` writer). |
| `src/pages/Chat/ChatPage.tsx` | Main chat shell. |
| `src/stores/` | Zustand stores for chat, settings, updater, tasks. |
| `src-tauri/src/` | Rust backend. |
| `src-tauri/src/agent/` | The AI loop. `mod.rs` has the system prompt and the OpenAI-style streaming. `checkpoint.rs` is the git snapshot machinery. `subagent.rs` is parallel sub-tasks. |
| `src-tauri/src/commands/` | Tauri IPC handlers. Each `*.rs` corresponds to a frontend `invoke()` call. |
| `src-tauri/src/tools/` | Tool implementations the AI can call: `read`, `write`, `edit`, `bash`, `grep`, `glob`. `file_lock.rs` is the per-file mutex + atomic-write helper. `path_sanity.rs` is the typo guard. |
| `src-tauri/src/storage/` | SQLite layer. `db.rs` has `ensure_schema` which is the source of truth for what columns must exist. |
| `src-tauri/src/util/no_window.rs` | Hides console-window flashes when spawning child processes on Windows. Apply via `.no_window()` on any `Command::new(...)`. |
| `src-tauri/migrations/` | Sqlx migrations. Note: historical 0002–0005 were consolidated into `ensure_schema`; only `0001_init.sql` remains. |
| `cliff.toml` | git-cliff config for release notes. |
| `scripts/` | Build/release helpers + the icon generator. |
| `docs/` | Long-form design docs (agents, governance, specs). |

## Data layout

Runtime files all live under `%APPDATA%\com.codefactory.app\` on Windows:

- `codefactory.db` — sqlite (sessions, messages, costs, checkpoints).
- `codefactory.db.backup-YYYYMMDD` — daily backup, 7-day rolling.
- `settings.json` — endpoints, model picks, permissions, hooks, MCP servers.

API keys are stored in the Windows Credential Manager via the `keyring`
crate, NOT on disk. Don't commit anything that looks like a key.

## Project memory (for the AI)

If a repo has a `.codefactory/memory.md`, the AI auto-injects its
contents into the system prompt every session. Use the **Remember**
button on any assistant message to append a fact. Legacy
`CODEFACTORY.md` at the project root is still read for backward
compatibility.

## Working with the conversational AI

The system prompt (see `SYSTEM_PROMPT` in `src-tauri/src/agent/mod.rs`)
enforces a few non-negotiable behaviours that are easy to inadvertently
weaken when editing the prompt:

1. **Plan-first for non-trivial work.** AI proposes before acting on
   anything multi-file.
2. **TDD execution loop.** Tests first, then implementation, until green.
3. **Test-modification discipline.** Editing a failing test purely to
   make it pass is forbidden; only a stated reason rooted in the spec
   justifies it.
4. **Communicate as an engineer, not a build log.** Plans and summaries
   lead with problem + approach; file lists go last and stay brief.

Don't relax these without thinking hard — they're the rails that let
the user trust the agent to act with minimal supervision.
