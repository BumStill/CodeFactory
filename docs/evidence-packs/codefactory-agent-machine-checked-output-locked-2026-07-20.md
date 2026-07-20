# CodeFactory Agent Machine-Checked Output Acceptance

## Scope

- Requirement: `CF-TB-R45`
- Product surface: shared `codefactory-agent-core`, desktop Agent completion gate, and product-policy headless Runtime
- Benchmark boundary: the acceptance uses a standalone Python CLI fixture under `/private/tmp`; it contains no Terminal-Bench task, verifier, repository fingerprint, expected benchmark answer, or task-specific branch.

## Failure-First Evidence

The released `v1.51.3` trajectory printed an actual value beside a different expected value, returned shell status `0`, and was still credited as successful verification. Its final executable Python heredoc also contained `test = ...`, which the shell classifier mistook for a `test` command.

Two shared-core tests failed before R45:

- `executable_interpreter_heredoc_cannot_masquerade_as_a_shell_test` classified the opaque Python heredoc as `Verification` instead of `Mutation`.
- `explicit_expected_behavior_requires_a_machine_checked_probe` allowed a plain zero-exit runtime with printed expected text to complete.

The fixed core also covers build-only checks, masked `grep -q ... || true`, assertions whose failure is swallowed by a later successful command, dedicated verifiers, and an end-to-end headless recovery where a premature final answer is rejected until a later shell equality assertion succeeds.

The post-fix Runtime reruns exposed two additional classifier defects. First, a compound command that wrote a workspace file and then ran a functional assertion was classified as Verification before mutation detection. Second, inline interpreter code using `open(..., 'w')` could modify a file without recording a Mutation. Failure-first tests reproduced both. Workspace writes now take priority over probes/assertions, and common inline interpreter file-write APIs are conservatively recognized, so each action invalidates earlier evidence and still requires a later independent verification.

## Locked Product Runtime

- Fixture: `/private/tmp/codefactory-product-eval-machine-assert-r45-v1`
- Public task: repair `cli.py`; `python3 cli.py 6` should output `42`; keep the CLI unchanged.
- Initial behavior: `python3 cli.py 6` printed `6`; independent shell equality assertion exited `1`.
- Provider/model: `deepseek / deepseek-v4-pro`
- Runtime policy: `product`
- Screen locked at start/end: `true / true`
- Status: `passed`
- Duration: `23,280 ms`
- Tool calls: `4`
- Model requests: `6`
- Prompt/completion/total tokens: `21,046 / 1,298 / 22,344`
- Completion evidence: `machine_checked_behavior_required=true`, `completed=true`, no blockers, last mutation/source-mutation sequence `3`, later machine-checked and successful verification sequence `4`
- Headless binary SHA-256: `889d36b602ae22f0f22b6812c1691b0d7a13da3a1216e58dacf2130a5519a5ff`
- Execution contract SHA-256: `422052c59515aace746fbf3bda2d0d3e9e6fb018b5c14c5635a6c7d1b8737c8b`
- Raw local evidence: `.codefactory/product-acceptance/machine-assert-r45-v6`

The final trajectory proves sequencing rather than only final success. Sequence `3` wrote `cli.py` and included a successful inline `if` check, but the workspace write made the whole action `Mutation`. CodeFactory did not complete until sequence `4` independently ran `result=$(python3 cli.py 6); test "$result" = "42"` and returned `0`. An independent terminal assertion after Runtime exit returned `MACHINE_ASSERT_OK`.

The earlier `.codefactory/product-acceptance/machine-assert-r45-v1` run is not acceptance evidence: it passed behaviorally, but its original instruction referred indirectly to README and recorded `machine_checked_behavior_required=false`. It was rejected and rerun with the explicit expected-output instruction above.

The later `v2` run was valid for an earlier candidate but became stale after classifier hardening. The `v3` run exposed the compound write-and-check misclassification because the modified file was real while `last_mutation_sequence` remained empty. The `v5` run then exposed an inline interpreter write recorded as ReadOnly. Both were rejected, fixed failure-first, and replaced by the exact-binary `v6` evidence above.

## Product Impact

This closes generic false-green paths in ordinary coding tasks. When a user states what a CLI, function, conversion, parser, or API should output or return, CodeFactory can no longer claim success from compilation, a zero-exit invocation, printed expected/actual text, or an assertion bundled into the same workspace-writing command. The final independent evidence must fail automatically when behavior differs.

## Delivery Boundary

This is local candidate evidence. It is not live until PR CI, remote real-App GUI, merge, deliberate release, published-artifact verification, and a released-build canary complete.
