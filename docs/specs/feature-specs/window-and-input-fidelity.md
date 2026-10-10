# 窗口位置与输入内容保真

### Background and user decision
CodeFactory is a coding tool: what the user types (commands, arguments, code) must reach the agent exactly as typed; the window should stay where the user left it, without hunting for it after every upgrade.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-WIN-R1 | After quitting and reopening (including upgrades), the main window goes back to its last position and size, on the same display | Test + one real restart check |
| CF-WIN-R2 | If that display is no longer connected or the saved position is off screen, open on the main display at a fully visible position; never open off-screen or across displays | Test |
| CF-INP-R1 | The input box does not auto-replace characters: `--`, straight quotes `"` `'`, `...`, etc. stay exactly as typed (no smart dashes / smart quotes / auto-correct); the stored message is byte-for-byte what the user entered | Table-driven tests + real-browser test (type in the input → check the sent content) |
| CF-INP-R2 | Pasted and typed text behave the same, and Chinese input-method composition is unaffected | Real-browser test |

### Applicable Harnesses
Spec Harness; Viewport Harness (input-box behaviour, real browser); Compatibility Harness (old versions have no saved window position); AI Collaboration Harness.

### Test matrix
- Window: normal restart / upgrade restart / external display unplugged / saved position off screen.
- Input: `--flag`, `"quote"`, `'single'`, `...`, mixed Chinese and English, pasted vs typed.

### 临时规则
按任务模板中的 2026-10-09 临时规则：只交付至 PR（`pr_only`），由用户负责观察 CI 并决定合并。
