# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-28T15-31-31Z`
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
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-153126`

## Blocker

The run did not start Harbor because provider credential lookup timed out.
Unlock or authorize the OS credential store, or launch with an explicit in-memory `CODEFACTORY_BENCH_API_KEY`.

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.50s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-153126

thread 'benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings' (3387800) panicked at src/benchmark.rs:2292:10:
start provider benchmark run: Other("Benchmark provider secret lookup timed out after 5s; unlock or authorize the OS credential store and retry")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... FAILED

failures:

failures:
    benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 169 filtered out; finished in 5.06s

error: test failed, to rerun pass `--lib`

```
