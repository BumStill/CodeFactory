# Terminal-Bench 2.1 Resource Preflight Aggregate Evidence

- generated_at: `2026-06-29`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- dataset: `terminal-bench/terminal-bench-2-1`
- subset: `terminal-bench-21-regression-subset-v1`
- model backend: `deepseek-v4-pro`

## Result

- run: `159041ce-5682-4835-843a-fbed9088aa9d`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T15-28-16Z.md`
- iteration report: `docs/evidence-packs/terminal-bench-21-iteration-2026-06-29T15-28-16Z.md`
- official comparable: `true`
- trials: `18`
- pass: `4 / 18`
- mean reward: `0.222`
- Harbor exceptions: `0`

## Interpretation

This is a real provider-backed aggregate improvement over the earlier fixed-subset run `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e`, which scored `0 / 18` with mean reward `0.000`.

It does not yet exceed the old offline projection baseline from the full run (`4 / 18`, mean reward `0.222222`). The current product target remains to exceed `4 / 18`, then move toward `7 / 18` before another full 89-task run.

## Infrastructure Change

- Local Colima/Docker was moved from `2 CPU / 4GB / 30GB overlay` to `4 CPU / 8GB / 99GB overlay`.
- The fixed subset runner now blocks provider-backed benchmark launch unless Docker CPU, memory, root free space, and apt bootstrap smoke pass.
- The same 18-task run completed without the earlier Docker CPU/container-start failure and without Harbor exceptions.

## Next Failure Targets

- `mteb-retrieve`: agent produced the expected `/app/result.txt`, but verifier dependency download failed with `UV_HTTP_TIMEOUT`; classifier now routes this shape to `environment/verifier-dependency-resource`.
- `query-optimize`: verifier spent long CPU time on slow SQL; next agent-loop improvement should require `EXPLAIN QUERY PLAN` / bounded timing before final SQL.
- `protein-assembly`: model generated oversized invalid JSON tool arguments; adapter should steer large file writes into chunked or script-generated commands before tool-call JSON breaks.
