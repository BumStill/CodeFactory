# CodeFactory Agent Policy-Denial Convergence Evidence

## 结论

- R50 已发布为 public `v1.52.1`，但精确 released-build `circuit-fibsqrt` canary 仍为 `0 / 1`。固定 18 题诚实总分仍是 `6 / 18`，本轮没有可报告的 Terminal-Bench 分数提升。
- R50 canary 将新的主产品缺陷暴露为：policy-denied tool batch 可以持续消耗模型轮次；文件存在断言可错误登记为目标输出验证；合法的 inline machine assertion 可能被分类为 ReadOnly 并在修复后被完成策略拒绝。
- R51 候选把修复落在共享 Rust Agent core、desktop OpenAI/Anthropic loop、headless runtime、benchmark evidence adapter 和普通产品验收驱动器，不包含 task name、固定答案、hidden verifier 或 task-specific repair。
- 锁屏 non-benchmark DeepSeek product acceptance 已完成真实失败、修复、非样例 assertion、同一 unittest 复跑和独立终端复验。当前状态仍为 `candidate, not live`，必须经过 PR/CI、刻意发版和 released-build canary 后才能声明上线。

## R50 发布与复评

- PR: `#145`
- merge commit: `9b1f7ac`
- Auto Release: `29800664412`
- Release: `29800677198`
- public tag: `v1.52.1`
- tag commit: `0fe33d56cbf6f4bb784b0b6581ba8dc4fed189cf`
- public release: `https://github.com/BumStill/CodeFactory/releases/tag/v1.52.1`
- published at: `2026-07-21T04:21:39Z`
- macOS DMG SHA-256: `987591cda8613a901e5f7f572a3a28d04020af061ef2b7f1cf03c2096ae67476`
- Windows setup SHA-256: `67efa7ac5f3e8f97354c301c81969d197b054d3f2ede5d63774c4608c3b1fa01`
- `latest.json` SHA-256: `1957c5b2ea5f9fe8376c9e68db3011f014af5e385a52b1f7b11e23e56c6bd4c6`
- independent download hashes matched GitHub metadata; `hdiutil verify` passed.

Exact released-build canary kept the task, model, concurrency, resources, verifier and timeout settings fixed:

- task: `terminal-bench/circuit-fibsqrt`
- model: `deepseek-v4-pro`
- concurrency: `1`
- outer timeout: `900s`
- lifecycle host cap: `780s`
- run: `43033375-c517-4f41-b512-3f2c3b42e476`
- reward: `0`
- failure class: `verification`
- usage: `30` model requests, `228,946` tokens, `15` external tools
- runtime: Agent stopped itself with exit `0`; runner did not intervene and no residual Docker process remained.
- completion blocker: requested output lacked a later machine-checked assertion linked to the target output.
- comparability boundary: Harbor trial import was comparable, but the one-task report is score-facing diagnostic only because the runner watchdog setting was enabled. It cannot update the fixed-18 aggregate.

R49 on the same task used `17` requests, `126,633` tokens and `12` tools. R50 therefore did not improve reward and consumed more model work.

## 真实根因

1. The canary trajectory contained `30` model responses but only `15` external tools. At least `11` non-external batches were policy-denied calls rather than productive tool execution.
2. Desktop and headless loops cleared `require_tool_next` when the model returned a tool call, before knowing whether any call actually executed. Repeated denied batches could therefore continue until the wall budget expired.
3. `test -f target && target example` could update machine-checked evidence even though the file assertion did not compare the target output with the requested value.
4. A legitimate `python3 -c` assertion was classified as ReadOnly. In the convergence window, the policy then rejected the exact verification it requested.
5. Multi-line inline assertions and `unittest && python3 -c` could be rejected because the shell exit-status detector treated quoted newlines as command separators.
6. `unittest; echo "EXIT: $?"` returns the status of `echo`, not the test. The policy correctly rejected it, but the recovery prompt did not tell the model how to preserve the verifier exit status.
7. Static command classification still records a temporary write-and-restore command as Mutation even when the final workspace is unchanged. This is documented as unfinished execution-effect work, not claimed as solved by R51.

## 通用产品修复

### 有界拒绝恢复

- A denied tool is not an executed action and does not clear the required-tool state.
- The first all-denied or otherwise non-executable batch emits `policy_denied_tool_batch`, records bounded `command/rule/reason`, and forces the next provider request to contain a tool.
- The recovery prompt requires a permitted bounded replacement and explicitly preserves verifier exit status: use a standalone verifier or fail-closed `&&`; do not append `; echo $?`, `|| true`, or `|| :`.
- A second consecutive non-executable batch stops incomplete instead of consuming the remaining model or wall budget.
- The same continuation state is used by headless, desktop OpenAI-compatible/ChatGPT and desktop Anthropic paths.

### 机器验证真实性

- File, executable, PID and port assertions remain preconditions; they do not prove requested behavior unless the target output is captured or piped into the assertion.
- Project tests and dedicated verifiers remain valid independent oracles.
- A non-mutating inline interpreter assertion with fail-closed exit status is Verification.
- Inline interpreter code that writes the workspace remains Mutation.
- Masked assertions such as `python3 -c "assert False"; echo done` and `python3 -c "assert False" || true` remain invalid verification.

### 可观测证据

- Headless JSONL protocol serializes `policy_denied_tool_batch` with decision records and cumulative usage.
- Benchmark trajectory and ordinary product acceptance trajectory retain only `command`, `rule`, and `reason`, cap count and field lengths, discard unknown fields, and redact credential values before writing evidence.
- This fixes the product acceptance evidence path as well as the benchmark adapter; the result is not benchmark-only observability.

## Failure-First 证据

The following regressions failed before their implementation change and passed afterward:

- `unrelated_file_assertion_does_not_machine_check_requested_output`: initially recorded an unrelated file assertion as machine evidence; now the sequence remains unset.
- `consecutive_policy_denials_keep_the_next_tool_choice_required`: the third provider request initially used `auto`; now it remains `required` after the first denied batch.
- `test_policy_denial_event_preserves_bounded_decisions`: initially raised `KeyError: decisions`; benchmark evidence now retains one bounded, allowlisted decision.
- `test_runtime_failure_persists_latest_usage_snapshot`: product acceptance trajectory initially raised `KeyError: decisions`; it now persists redacted denial decisions and latest usage.
- `multiline_inline_assertion_in_final_and_chain_controls_exit_status`: initially failed because a legal multi-line inline assertion was not machine checked, then failed again because the pure assertion was classified `ReadOnly`; both paths now pass as Verification.
- `tool_denial_recovery_prompt_preserves_verifier_exit_status`: initially lacked standalone/`&&`/masking guidance; the recovery prompt now names all three constraints.

## 非 Benchmark 产品验收

Fixture: `/private/tmp/codefactory-product-eval-failure-repair-r50-v1`

Instruction: repair a slug normalizer so the existing tests pass, diagnose the failure, make the smallest implementation change, and rerun the same unittest.

Initial independent failure:

```text
AssertionError: 'release\t-candidate' != 'release-candidate'
FAILED (failures=1)
```

Intermediate runs remained truthful:

- v3 stopped incomplete after a valid extra assertion exposed leading/trailing dash behavior and later policy denials remained unresolved.
- v4 rejected `unittest; echo "EXIT: $?"` because the suffix masked the test status.
- v5 repaired the existing test but kept `completed=false` after a broader extra check failed and was not replayed at the same scope; the model's text claim did not override structured evidence.

Final v6 implementation proof:

- provider/model: `deepseek / deepseek-v4-pro`
- proof tier: `agent-runtime-no-gui`
- screen: locked at start and end
- workspace isolation: `macos-sandbox-exec`
- duration: `45,169ms`
- model requests: `9`
- total tokens: `45,533`
- external tools: `8`
- mutation sequence: `6`
- independent machine-check sequence: `8`
- successful project-test sequence: `8`
- completion: `true`
- blockers: none
- failed verification fingerprint: none

The v6 trajectory executed six non-example assertions for mixed spaces, tabs, repeated hyphens, unchanged values, uppercase input and empty input, then reran the unchanged unittest. It used the pre-clarification contract hash and is retained as behavioral evidence.

Final v7 exact-contract proof after the specification/contract update:

- provider/model: `deepseek / deepseek-v4-pro`
- proof tier: `agent-runtime-no-gui`
- screen: locked at start and end
- duration: `22,269ms`
- model requests: `9`
- total tokens: `46,346`
- external tools: `9`
- mutation sequence: `8`
- successful project-test and independent verification sequence: `9`
- completion: `true`
- blockers: none
- failed verification fingerprint: none
- execution contract SHA-256 in result: `b7371da29d18ef8b3a7ab4d1bdf3254f312dc2b3830d5b918b5f3e1d5c8ddcd7`

The v7 trajectory reproduced the unchanged unittest failure, made the source mutation, and reran the same unittest successfully. An independent terminal rerun passed `1 / 1`, and a separate independent edge assertion command covered mixed spaces, tabs, repeated hyphens and empty input.

Current candidate hashes:

- headless binary SHA-256: `11a0464d8da043b51635bf81941c7e2b9321fa47ad9040d88bfdf11a745dc1dd`
- execution contract SHA-256: `b7371da29d18ef8b3a7ab4d1bdf3254f312dc2b3830d5b918b5f3e1d5c8ddcd7`

## 产品价值示例

- CLI repair: a denied read or masked test cannot silently consume all remaining rounds; CodeFactory asks for one executable replacement and stops honestly if it still cannot run.
- Data transformation: `python3 -c` assertions over non-example inputs execute as verification instead of being rejected as inspection.
- API/service work: PID or port existence cannot replace the requested response assertion; the real output must flow into a test or verifier.
- Auditability: a failed run now tells the user which command was rejected by which rule and why, without persisting the API key.

## 发布边界与下一步

R51 is not live. Required next steps are:

1. Complete independent P1/P2 review and all local Rust, Python, frontend, build and governance gates.
2. Deliver through branch sync, PR, CI and merge.
3. Trigger deliberate Auto Release and verify public macOS/Windows artifacts.
4. Rerun the exact released-build canary with fixed parameters.
5. Only after task-level proof, rerun fixed 18. The first gate remains `>=16 / 18` with zero regression across the honest pass set; final target remains `18 / 18`.

Unfinished product work remains execution-effect-aware mutation tracking and a redacted actionable replay obligation for unresolved failed verification scope.
