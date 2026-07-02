# Terminal-Bench 2.1 Regression Subset Baseline

- generated_at: `2026-06-28T15-41-50Z`
- source_job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- subset: `terminal-bench-21-regression-subset-v1`
- dataset: `terminal-bench/terminal-bench-2-1`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- model_backend: `deepseek-v4-pro`
- task_count: `18`
- pass_count: `4`
- mean_reward: `0.222222`
- level: `early scaffold baseline`
- missing_usage_or_cost_trials: `18`

This is an offline subset projection from the completed full Harbor job, not a fresh provider-backed rerun.

## Failure Class Counts

| Failure class | Count |
| --- | ---: |
| `environment` | `2` |
| `long-horizon` | `4` |
| `pass` | `4` |
| `tool-use` | `3` |
| `verification` | `5` |

## Failure Reason Counts

| Failure reason | Count |
| --- | ---: |
| `VerifierTimeoutError` | `1` |
| `command-timeout` | `4` |
| `docker-cpu-limit` | `1` |
| `harbor-tests-upload` | `1` |
| `pass` | `4` |
| `tool-use` | `3` |
| `verifier-zero` | `4` |

## Selection Bucket Counts

| Bucket | Count |
| --- | ---: |
| `command-timeout` | `4` |
| `docker-resource` | `1` |
| `harbor-tests-upload` | `1` |
| `passed-smoke` | `4` |
| `tool-use` | `3` |
| `verifier-timeout` | `1` |
| `verifier-zero` | `4` |

## Trials

| Task | Reward | Failure class | Failure reason | Bucket | Tool calls | Trial dir |
| --- | ---: | --- | --- | --- | ---: | --- |
| `write-compressor` | `1.0` | `pass` | `pass` | `passed-smoke` | `18` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/write-compressor__Vp4hRre` |
| `extract-elf` | `1.0` | `pass` | `pass` | `passed-smoke` | `46` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/extract-elf__LzBcZZR` |
| `filter-js-from-html` | `1.0` | `pass` | `pass` | `passed-smoke` | `35` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/filter-js-from-html__SAMhuwj` |
| `nginx-request-logging` | `1.0` | `pass` | `pass` | `passed-smoke` | `42` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/nginx-request-logging__ayaCPQ8` |
| `circuit-fibsqrt` | `0.0` | `verification` | `verifier-zero` | `verifier-zero` | `20` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/circuit-fibsqrt__ibNdC9k` |
| `configure-git-webserver` | `0.0` | `verification` | `verifier-zero` | `verifier-zero` | `53` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/configure-git-webserver__B55Zv2H` |
| `mteb-retrieve` | `0.0` | `verification` | `verifier-zero` | `verifier-zero` | `36` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/mteb-retrieve__b6sxiNN` |
| `sanitize-git-repo` | `0.0` | `verification` | `verifier-zero` | `verifier-zero` | `48` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/sanitize-git-repo__efJzmG8` |
| `query-optimize` | `0.0` | `verification` | `VerifierTimeoutError` | `verifier-timeout` | `31` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/query-optimize__SJ3STAX` |
| `count-dataset-tokens` | `0.0` | `tool-use` | `tool-use` | `tool-use` | `42` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/count-dataset-tokens__uZTcZYe` |
| `install-windows-3.11` | `0.0` | `tool-use` | `tool-use` | `tool-use` | `35` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/install-windows-3.11__jXLizr2` |
| `protein-assembly` | `0.0` | `tool-use` | `tool-use` | `tool-use` | `45` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/protein-assembly__nTde9Fh` |
| `build-cython-ext` | `0.0` | `long-horizon` | `command-timeout` | `command-timeout` | `` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/build-cython-ext__rcGWm54` |
| `kv-store-grpc` | `0.0` | `long-horizon` | `command-timeout` | `command-timeout` | `` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/kv-store-grpc__CqnyN3e` |
| `sparql-university` | `0.0` | `long-horizon` | `command-timeout` | `command-timeout` | `` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/sparql-university__9sbS53N` |
| `torch-tensor-parallelism` | `0.0` | `long-horizon` | `command-timeout` | `command-timeout` | `` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/torch-tensor-parallelism__imbAEv6` |
| `caffe-cifar-10` | `0.0` | `environment` | `docker-cpu-limit` | `docker-resource` | `` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/caffe-cifar-10__A4FzCQZ` |
| `qemu-startup` | `0.0` | `environment` | `harbor-tests-upload` | `harbor-tests-upload` | `35` | `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422/qemu-startup__HG7LesF` |

## Score-Driven Improvement Direction

- P0: keep evaluation infrastructure separate from agent capability by treating credential, Docker resource, and Harbor upload failures as blockers or environment failures.
- P1: reduce exception/timeout outcomes first; target `command-timeout`, service lifecycle, and long command supervision so failures reach verifier output instead of aborting trials.
- P2: convert verifier-zero tasks into structured repair goals with expected artifact, failing assertion, smallest rerun command, and final-before-verify gate.
- P3: move tool-use errors into a planner policy with cwd/file inventory, background process templates, and automatic alternatives for missing commands/files.
- P4: only rerun the full 89-task benchmark after this 18-task subset improves from `4 / 18` to at least `7 / 18` under the same attribution axis.
