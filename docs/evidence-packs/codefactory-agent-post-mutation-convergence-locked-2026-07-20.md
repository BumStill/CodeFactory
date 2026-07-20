# CodeFactory Agent Post-Mutation Convergence Acceptance

## Scope

- Requirement: `CF-TB-R44`
- Product surface: shared `codefactory-agent-core`, desktop Agent progress prompt, and product-policy headless Runtime
- Benchmark boundary: this acceptance uses a standalone Python fixture under `/private/tmp`; it contains no Terminal-Bench task, verifier, repository fingerprint, expected benchmark answer, or task-specific branch.

## Failure-First Evidence

The previous `ProgressTracker` stopped counting successful ReadOnly and RuntimeProbe outcomes after the first mutation. Two new shared-core tests failed before the implementation change:

- `mutation_starts_a_new_bounded_read_only_inspection_window` unwrapped `None` after the post-mutation read limit.
- `functional_probe_resets_post_mutation_inspection_pressure` did not produce pressure after a functional probe opened the next inspection window.
- `failed_read_only_outcome_does_not_reset_inspection_pressure` showed that a missing-file read could reset the window and permit alternating successful/failed inspection loops.

The fixed tracker applies the bounded window before and after mutation. Headless additionally has an end-to-end fake-provider protocol test, `post_mutation_inspection_budget_forces_action_before_more_reads`: after a mutation and four reads, a fifth pure read is returned to the model as an `inspection_budget` denial instead of reaching the tool bridge; the model then issues a bounded loopback functional probe and completes only after its success.

## Locked Product Runtime

- Fixture: `/private/tmp/codefactory-product-eval-post-mutation-window-v1`
- Public task: repair `processor.py` so positive values are deduplicated, squared, and sorted; do not modify `verify.py`; run `python3 verify.py`.
- Provider/model: `deepseek / deepseek-v4-pro`
- Runtime policy: `product`
- Screen locked at start/end: `true / true`
- Status: `passed`
- Duration: `17,774 ms`
- Tool calls: `6`
- Model requests: `4`
- Prompt/completion/total tokens: `10,917 / 980 / 11,897`
- Completion evidence: `completed=true`, no blockers, mutation sequence `5`, later verification sequence `6`
- Headless binary SHA-256: `a6f5165c0a19d5c46048fe625c0bd1d7e8660cc7372a4470852a73c63f0bcf21`
- Execution contract SHA-256: `efff1c555d4eecf58d550e398193c69e694eba54668025d95b717a4a4862a241`
- Raw local evidence: `.codefactory/product-acceptance/post-mutation-window-r44-v2`

The Agent changed only the implementation behavior needed by the public fixture. An independent terminal invocation after Runtime exit returned `PUBLIC_ACCEPTANCE_OK`.

## Product Impact

This change benefits ordinary long coding tasks where the Agent has already made a candidate edit but begins repeatedly rereading source instead of repairing or testing. The first mutation no longer grants unlimited later inspection. Each mutation or functional probe opens a new bounded read window; when exhausted, the next action must be a corrective mutation or bounded functional verification.

## Delivery Boundary

This is local candidate evidence. It is not live until PR CI, remote real-App GUI, merge, deliberate release, published-artifact verification, and a released-build long-task canary complete.
