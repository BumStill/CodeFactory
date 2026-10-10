# 本地非界面派单入口（锁屏也能派单）

### Background and user decision
The user expects CodeFactory to keep working unattended for long stretches (e.g. overnight) while an orchestrator (Claude/Codex) keeps dispatching tasks to it. Dispatching must not depend on the screen being unlocked, and must not weaken any existing safety boundary.

### Decision
Provide a **local-only** task entry point that the running CodeFactory accepts and that works without the GUI. Tasks submitted through it go exactly the same way as ones typed in the GUI (same session/objective/permission/delivery/completion gates); there's no bypass channel.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-HDE-R1 | Through this entry point, the local user can: create a session in a named project and send the first message; send a message to a named existing session; set that session's permission mode (standard/trusted); query a session's state (objective state, latest reply summary, PR number). Works when the screen is locked or the window is on another display | Integration tests + one real-machine record: submitting a task with the screen locked succeeds |
| CF-HDE-R6 | **Everything an orchestrator needs can be done from the background** (user's default requirement, 2026-10-10): through this entry point you can also — pick the model when creating a session and change an existing session's model / set the default model; switch the session currently shown in the GUI (M27: menu switching doesn't work); stop the current run; list and approve/deny pending approval requests (approving must still go through the same permission rules: high-risk ones are only approved after an explicit choice, no blanket approval); move the main window back to the main display (M42). None of these depend on a pop-up menu, coordinate clicks or the screen being unlocked | Integration test for each capability + one real-machine record with the screen locked |
| CF-HDE-R2 | **Local only, no remote attack surface**: no network port is opened; only the current OS user can submit (enforced via file permissions or equivalent); requests from other users / other machines are rejected | Counter-example tests: other users / world-writable path / any network access → rejected |
| CF-HDE-R3 | **Doesn't weaken anything**: messages submitted this way go through exactly the same admission as GUI input (wording-inferred permissions, delivery authorization, permission prompts, completion gate); delivery authorization must be stated explicitly in the request (a structured field, not inferred from the wording, see M48); high-risk operations still need confirmation | Tests: same input via GUI and via this entry point produces identical admission results |
| CF-HDE-R4 | Every submission is traceable: who (local entry point) / when / which session / the original text, written to the audit log; the session shows "this message came from the local task entry point" | Test + plain-language wording |
| CF-HDE-R5 | Usage docs: one command or one example file can submit a task; the docs state the security boundary | Docs + a runnable example |

### Applicable Harnesses
Spec Harness; Compatibility Harness; Observation Harness (submission audit); AI Collaboration Harness. **Touches a security boundary**: the PR must have a dedicated "Threat model" section (who can call it, what they can do, how it's limited), and counter-example tests are the focus of acceptance.

### Test matrix
- Normal: screen locked → create session + send + set trusted + query state.
- Counter-examples: another user, a file with wrong permissions, a request with no delivery authorization trying to deliver, a non-existent project/session.
- Consistency: GUI vs this entry point, admission results identical.

## Implementation Notes

These notes add technical constraints only. They do not change any requirement,
acceptance criterion or minimum evidence above.

### Transport
A **Unix domain socket** at `<app data dir>/dispatch.sock`, created with mode
`0600` inside a `0700` directory. No TCP/UDP listener is ever opened. On Windows
the equivalent private named pipe is not implemented in this change — see the
leftover section of the PR.

### Protocol
One JSON object per line, one response line per connection. `operation` is the
discriminator; the schema is closed (`deny_unknown_fields`). `delivery_authorized`
is a **required** boolean with no default, so an unstated delivery request does
not parse at all.

Operations: `create_and_send`, `send`, `set_permission`, `status`, `set_model`,
`switch_session`, `stop`, `list_approvals`, `resolve_approval`,
`focus_main_display`.

### Who executes what
Transport and security live in Rust (`headless_dispatch`). Behaviour lives where
the GUI's behaviour already lives: the app forwards each request to the interface
over one event and waits for the reply, so the entry point cannot drift from the
GUI code path. `focus_main_display` is handled natively in Rust (window
placement) because it has no interface-side equivalent.

### Audit
Every **parsed** request is appended to `<app data dir>/dispatch-audit.log`
(JSON lines, mode `0600`) before it is executed: timestamp, source, operation,
session, project and the original text.

### Entry-point routing
`src-tauri/src/main.rs` is the E2E-001 trust root (a `DRIVER_FILES` member in
`tools/governance/scenario_case_execution.py`): a normal PR must keep it
byte-identical, and it must keep dispatching to the canonical unattended module
first. The Rust client mode (`CodeFactory --dispatch '<json>'`), therefore, is
**not** a new branch in `main.rs`; it is resolved at the top of the library app
entry `lib.rs::run()` — the one place the frozen `main` falls through to — before
any Tauri/GUI work. The socket client lives in `headless_dispatch::run_client_cli`.
The documented, orchestrator-facing command remains `node scripts/dispatch-task.mjs`,
which speaks the same protocol directly to the private socket.
