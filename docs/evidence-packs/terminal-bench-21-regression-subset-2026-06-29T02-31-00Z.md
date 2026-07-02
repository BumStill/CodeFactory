# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-29T02-31-00Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `101`
- explicit_key_present: `no`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `4`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-023050`

## Result

The provider bridge command did not import a completed Harbor job.

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.58s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-023050
provider_bridge_result status=failed exit_code=Some(1) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-023050
provider_bridge_stdout_tail:

provider_bridge_stderr_tail:
[truncated to last 4000 chars]
│ ❱ 305 │   │   │   │   await dataset.get_task_configs(                        │
│   306 │   │   │   │   │   disable_verification=config.verifier.disable       │
│   307 │   │   │   │   )                                                      │
│   308 │   │   │   )                                                          │
│                                                                              │
│ /Users/leo/.local/share/uv/tools/harbor/lib/python3.12/site-packages/harbor/ │
│ models/job/config.py:161 in get_task_configs                                 │
│                                                                              │
│   158 │   │   elif self.is_local():                                          │
│   159 │   │   │   return await                                               │
│       self._get_local_task_configs(disable_verification)                     │
│   160 │   │   elif self.is_package():                                        │
│ ❱ 161 │   │   │   return await self._get_package_task_configs()              │
│   162 │   │   else:                                                          │
│   163 │   │   │   return await self._get_registry_task_configs()             │
│   164                                                                        │
│                                                                              │
│ /Users/leo/.local/share/uv/tools/harbor/lib/python3.12/site-packages/harbor/ │
│ models/job/config.py:275 in _get_package_task_configs                        │
│                                                                              │
│   272 │   │   │   │   download_dir=self.download_dir,                        │
│   273 │   │   │   │   source=self.name,                                      │
│   274 │   │   │   )                                                          │
│ ❱ 275 │   │   │   for task_id in self._filter_task_ids(metadata.task_ids)    │
│   276 │   │   │   if isinstance(task_id, PackageTaskId)                      │
│   277 │   │   ]                                                              │
│   278                                                                        │
│                                                                              │
│ /Users/leo/.local/share/uv/tools/harbor/lib/python3.12/site-packages/harbor/ │
│ models/job/config.py:132 in _filter_task_ids                                 │
│                                                                              │
│   129 │   │   │   ]                                                          │
│   130 │   │   │   if not filtered_ids:                                       │
│   131 │   │   │   │   available = sorted(tid.get_name() for tid in task_ids) │
│ ❱ 132 │   │   │   │   raise ValueError(                                      │
│   133 │   │   │   │   │   f"No tasks matched the filter(s)                   │
│       {self.task_names}. "                                                   │
│   134 │   │   │   │   │   f"There are {len(available)} tasks available in    │
│       this dataset. "                                                        │
│   135 │   │   │   │   │   f"Example task names: {available[:5]}"             │
╰──────────────────────────────────────────────────────────────────────────────╯
ValueError: No tasks matched the filter(s) ['write-compressor', 'extract-elf', 
'filter-js-from-html', 'nginx-request-logging', 'circuit-fibsqrt', 
'configure-git-webserver', 'mteb-retrieve', 'sanitize-git-repo', 
'query-optimize', 'count-dataset-tokens', 'install-windows-3.11', 
'protein-assembly', 'build-cython-ext', 'kv-store-grpc', 'sparql-university', 
'torch-tensor-parallelism', 'caffe-cifar-10', 'qemu-startup']. There are 89 
tasks available in this dataset. Example task names: 
['terminal-bench/adaptive-rejection-sampler', 'terminal-bench/bn-fit-modify', 
'terminal-bench/break-filter-js-from-html', 'terminal-bench/build-cython-ext', 
'terminal-bench/build-pmars']

thread 'benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings' (3900744) panicked at src/benchmark.rs:2387:9:
assertion `left == right` failed: Harbor provider run failed
  left: "failed"
 right: "completed"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... FAILED

failures:

failures:
    benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 170 filtered out; finished in 10.07s

error: test failed, to rerun pass `--lib`

```
