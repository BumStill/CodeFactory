# Terminal-Bench 2.1 Evaluation Long Task

## Basics
- Task ID: CF-LT-TB21
- Title: Terminal-Bench 2.1 ability evaluation system
- Feature spec: `docs/specs/feature-specs/terminal-bench-21-evaluation.md`
- Related Req IDs: CF-TB-R1, CF-TB-R2, CF-TB-R3, CF-TB-R4, CF-TB-R5, CF-TB-R6

## Completion Standard
- Done means: CodeFactory can run or import a Terminal-Bench 2.1 Harbor job, persist run/trial/artifact evidence, classify failures, show capability profile, and compare a regression subset across builds with real evidence.
- Blocked means: Harbor/Docker/dataset/agent-adapter/runtime access prevents real smoke verification, with exact command, error, and next action recorded.

## Current State
- Current phase: design
- Current checkpoint: business, architecture, UX, and feature spec drafted for review.
- Next owner: planning / system engineering
- Updated at: 2026-06-27

## Completed Items
- Verified current external Terminal-Bench 2.1 run surface from official sources.
- Confirmed Terminal-Bench 2.1 uses Harbor dataset `terminal-bench/terminal-bench-2-1`.
- Confirmed official leaderboard marks Terminal-Bench 2.1 live and notes submissions may not modify timeouts/resources.
- Drafted business design, architecture design, UX design, and feature spec.

## Remaining Items
- Implement benchmark profile and environment probe.
- Implement Harbor job importer with fake fixture coverage.
- Implement CodeFactory headless agent adapter for Harbor.
- Implement `benchmark-sandbox` policy preset with hard host/secret boundaries.
- Add Benchmarks UI for run summary, trial details, failure triage, and capability profile.
- Run real Harbor smoke evaluation and import the result.
- Compare at least one baseline/head subset after an implementation change.

## Blockers
- No CodeFactory headless runner or Harbor adapter exists yet.
- Real Terminal-Bench 2.1 smoke verification has not been run in this branch.
- Official leaderboard submission process is separate from local evaluation and not covered by this design-only slice.

## Evidence
- Local evidence: design docs and feature spec added in this branch.
- Release evidence: not live.
- Blocking evidence: current slice is docs-only; no Harbor smoke run or adapter implementation exists yet.

## AI Collaboration
- context scope: CodeFactory repo docs, AGENTS rules, current official Terminal-Bench and Harbor docs.
- assumptions: Terminal-Bench 2.1 should be treated as the primary external terminal-agent benchmark; CodeFactory must add headless execution rather than rely on desktop UI approval.
- review point: user/product review of this design package before implementation.
- validation result: governance and long-task validators should pass for this design package.

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
