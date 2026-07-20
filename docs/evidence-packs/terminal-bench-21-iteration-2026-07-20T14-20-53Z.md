# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-07-20T14-20-53Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `regression`
- subset_path: `/Users/leo/Projects/CodeFactory-tb21-v1492/docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- hypothesis: `Released v1.49.2 establishes a reproducible fixed-18 baseline after clean-checkout sidecar build and bidirectional Docker job-root validation.`
- target_failure_class: `long-horizon`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-14T03-59-42Z.md`
- run: `677cbb1a-b24b-422b-8a64-d4895b15af07`
- pass_count: `6`
- trials: `18`
- mean_reward: `0.333`

## Head

- path: `/Users/leo/Projects/CodeFactory-tb21-v1492/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T14-20-53Z.md`
- run: `dd54bc63-0f54-4243-ba0e-a1f5d81b9562`
- pass_count: `6`
- trials: `18`
- mean_reward: `0.333`

## Product Capability Impact

- verdict: benchmark-only
- capability: Measure the released CodeFactory Agent path without task-specific hints; the runner changes make the evaluation reproducible and prevent invalid infrastructure states from becoming scores.
- non_benchmark_example: A normal CodeFactory user asks the Agent to implement a multi-step repository task; the evaluated subject remains the v1.49.2 shared Rust Agent path.
- benchmark_only_boundary: This iteration changes only benchmark launch and evidence integrity; it does not change prompts, task handling, verifier access, or product Agent behavior.

## Delta

- comparable_delta: `no`
- reason: baseline evidence is marked non-comparable.

## Failure Class Counts

Baseline:
- `long-horizon`: `8`
- `pass`: `6`
- `verification`: `4`

Head:
- `model-provider`: `2`
- `pass`: `6`
- `verification`: `10`

## Next Improvement Queue

- P0: inspect the dominant failure class and choose one targeted canary before broader regression.
- P1: rerun the fixed subset only after the targeted canary shows a behavior delta.

## Command Output Tail

```text
iling time v0.3.47
   Compiling dyn-clone v1.0.20
   Compiling foldhash v0.2.0
   Compiling base64 v0.21.7
   Compiling dom_query v0.27.0
   Compiling infer v0.19.0
   Compiling serde-untagged v0.1.9
   Compiling cargo_metadata v0.19.2
   Compiling json-patch v3.0.1
   Compiling serde_with v3.20.0
   Compiling objc2-exception-helper v0.1.1
   Compiling plist v1.9.0
   Compiling option-ext v0.2.0
   Compiling tokio-macros v2.7.0
   Compiling socket2 v0.6.3
   Compiling tinystr v0.8.3
   Compiling potential_utf v0.1.5
   Compiling icu_locale_core v2.2.0
   Compiling icu_collections v2.2.0
   Compiling mio v1.2.0
   Compiling objc2 v0.6.4
   Compiling objc2-encode v4.1.0
   Compiling crc32fast v1.5.0
   Compiling tokio v1.52.3
   Compiling simd-adler32 v0.3.9
   Compiling ring v0.17.14
   Compiling icu_provider v2.2.0
   Compiling heck v0.5.0
   Compiling icu_normalizer v2.2.0
   Compiling icu_properties v2.2.0
   Compiling futures-io v0.3.32
   Compiling crossbeam-utils v0.8.21
   Compiling adler2 v2.0.1
   Compiling idna_adapter v1.2.2
   Compiling futures-task v0.3.32
   Compiling idna v1.1.0
   Compiling miniz_oxide v0.8.9
   Compiling url v2.5.8
   Compiling urlpattern v0.3.0
   Compiling tracing-attributes v0.1.31
   Compiling rustc_version v0.4.1
   Compiling core-foundation v0.10.1
   Compiling toml_datetime v0.7.5+spec-1.1.0
   Compiling winnow v0.7.15
   Compiling untrusted v0.9.0
   Compiling embed-resource v3.0.9
   Compiling dirs-sys v0.5.0
   Compiling dirs v6.0.0
   Compiling tauri-winres v0.3.6
   Compiling rustls v0.23.40
   Compiling pkg-config v0.3.33
   Compiling cpufeatures v0.2.17
   Compiling tracing-core v0.1.36
   Compiling subtle v2.6.1
   Compiling toml v0.9.12+spec-1.1.0
   Compiling vcpkg v0.2.15
   Compiling cargo_toml v0.22.3
   Compiling sha2 v0.10.9
   Compiling futures-macro v0.3.32
   Compiling block2 v0.6.2
   Compiling objc2-core-foundation v0.3.2
   Compiling time-macros v0.2.27
   Compiling zlib-rs v0.6.3
   Compiling tracing v0.1.44
   Compiling futures-util v0.3.32
   Compiling tauri-utils v2.9.1
   Compiling objc2-foundation v0.3.2
   Compiling fdeflate v0.3.7
   Compiling num-traits v0.2.19
   Compiling bitflags v1.3.2
   Compiling raw-window-handle v0.6.2
   Compiling dpi v0.1.2
   Compiling cookie v0.18.1
   Compiling getrandom v0.2.17
   Compiling flate2 v1.1.9
   Compiling foreign-types-macros v0.2.3
   Compiling foreign-types-shared v0.3.1
   Compiling foreign-types v0.5.0
   Compiling png v0.17.16
   Compiling rustls-webpki v0.103.13
   Compiling dispatch2 v0.3.1
   Compiling crossbeam-channel v0.5.15
   Compiling futures-channel v0.3.32
   Compiling core-graphics-types v0.2.0
   Compiling security-framework-sys v2.17.0
   Compiling iana-time-zone v0.1.65
   Compiling tauri-runtime v2.11.1
   Compiling futures-sink v0.3.32
   Compiling allocator-api2 v0.2.21
   Compiling foldhash v0.1.5
   Compiling wry v0.55.1
   Compiling parking v2.2.1
   Compiling security-framework v3.7.0
   Compiling core-graphics v0.25.0
   Compiling hashbrown v0.15.5
   Compiling ico v0.5.0
   Compiling rustix v1.1.4
   Compiling tokio-util v0.7.18
   Compiling webpki-roots v1.0.7
   Compiling getrandom v0.3.4
   Compiling crc-catalog v2.5.0
   Compiling unicode-segmentation v1.13.2
   Compiling tauri-runtime-wry v2.11.1
   Compiling keyboard-types v0.7.0
   Compiling h2 v0.4.14
   Compiling crc v3.4.0
   Compiling webpki-roots v0.26.11
   Compiling hashlink v0.10.0
   Compiling system-configuration-sys v0.6.0
   Compiling tauri-plugin v2.6.1
   Compiling tauri-build v2.6.1
   Compiling tauri-codegen v2.6.1
   Compiling png v0.18.1
   Compiling libsqlite3-sys v0.30.1
   Compiling serialize-to-javascript-impl v0.1.2
   Compiling core-foundation v0.9.4
   Compiling spin v0.9.8
   Compiling slab v0.4.12
   Compiling system-configuration v0.7.0
   Compiling serialize-to-javascript v0.1.2
   Compiling concurrent-queue v2.5.0
   Compiling hyper v1.9.0
   Compiling serde_repr v0.1.20
   Compiling embed_plist v1.2.2
   Compiling hyper-util v0.1.20
   Compiling tempfile v3.27.0
   Compiling event-listener v5.4.1
   Compiling crossbeam-queue v0.3.12
   Compiling chrono v0.4.44
   Compiling atoi v2.0.0
   Compiling libz-sys v1.1.28
   Compiling either v1.15.0
   Compiling futures-intrusive v0.5.0
   Compiling tokio-stream v0.1.18
   Compiling bumpalo v3.20.2
   Compiling objc2-app-kit v0.3.2
   Compiling signal-hook v0.3.18
   Compiling zopfli v0.8.3
   Compiling tower v0.5.3
   Compiling futures-executor v0.3.32
   Compiling flume v0.11.1
   Compiling tauri v2.11.1
   Compiling tauri-macros v2.6.1
   Compiling tauri-plugin-fs v2.5.1
   Compiling serde_urlencoded v0.7.1
   Compiling memoffset v0.6.5
   Compiling zip v2.4.2
   Compiling tauri-plugin-process v2.3.1
   Compiling tauri-plugin-dialog v2.7.1
   Compiling tauri-plugin-shell v2.3.5
   Compiling sqlx-core v0.8.6
   Compiling tauri-plugin-updater v2.10.1
   Compiling tower-http v0.6.10
   Compiling native-tls v0.2.18
   Compiling tokio-rustls v0.26.4
   Compiling libgit2-sys v0.17.0+1.8.1
   Compiling serial-core v0.4.0
   Compiling ioctl-rs v0.1.6
   Compiling termios v0.2.2
   Compiling os_pipe v1.2.3
   Compiling rfd v0.16.0
   Compiling hex v0.4.3
   Compiling dotenvy v0.15.7
   Compiling sigchld v0.2.4
   Compiling sqlx-sqlite v0.8.6
   Compiling serial-unix v0.4.0
   Compiling hyper-rustls v0.27.9
   Compiling objc2-web-kit v0.3.2
   Compiling tao v0.35.2
   Compiling muda v0.19.1
   Compiling window-vibrancy v0.6.0
   Compiling sqlx-macros-core v0.8.6
   Compiling objc2-osa-kit v0.3.2
   Compiling tokio-native-tls v0.3.1
   Compiling rustls-platform-verifier v0.7.0
   Compiling xattr v1.6.1
   Compiling filetime v0.2.29
   Compiling pathdiff v0.2.3
   Compiling lazy_static v1.5.0
   Compiling pin-utils v0.1.0
   Compiling sharded-slab v0.1.7
   Compiling nix v0.25.1
   Compiling tar v0.4.46
   Compiling open v5.3.5
   Compiling reqwest v0.13.3
   Compiling osakit v0.3.1
   Compiling hyper-tls v0.6.0
   Compiling sqlx-macros v0.8.6
   Compiling serial v0.4.0
   Compiling shared_child v1.1.1
   Compiling codefactory v1.49.2 (/Users/leo/Projects/CodeFactory-tb21-v1492/src-tauri)
   Compiling codepage v0.1.2
   Compiling quick-xml v0.31.0
   Compiling matchers v0.2.0
   Compiling tracing-log v0.2.0
   Compiling dirs-sys v0.4.1
   Compiling filedescriptor v0.8.3
   Compiling bstr v1.12.1
   Compiling thread_local v1.1.9
   Compiling shell-words v1.1.1
   Compiling hashbrown v0.14.5
   Compiling minisign-verify v0.2.5
   Compiling nu-ansi-term v0.50.3
   Compiling downcast-rs v1.2.1
   Compiling tracing-subscriber v0.3.23
   Compiling globset v0.4.18
   Compiling sqlx v0.8.6
   Compiling dashmap v6.2.1
   Compiling portable-pty v0.8.1
   Compiling calamine v0.26.1
   Compiling dirs v5.0.1
   Compiling reqwest v0.12.28
   Compiling rust_xlsxwriter v0.79.4
   Compiling zip v4.6.1
   Compiling codefactory-agent-core v0.1.0 (/Users/leo/Projects/CodeFactory-tb21-v1492/src-tauri/crates/agent-core)
   Compiling keyring v3.6.3
   Compiling git2 v0.19.0
warning: multiple fields are never read
   --> src/agent/journal.rs:133:9
    |
131 | pub struct JournalRow {
    |            ---------- fields in this struct
132 |     pub task_id: String,
133 |     pub session_id: String,
    |         ^^^^^^^^^^
134 |     pub hash_version: i64,
135 |     pub local_digest: String,
    |         ^^^^^^^^^^^^
136 |     pub dispatch_key: String,
137 |     pub dep_keys_json: String,
    |         ^^^^^^^^^^^^^
138 |     pub resolved_model: String,
    |         ^^^^^^^^^^^^^^
139 |     pub resolved_tools_json: String,
    |         ^^^^^^^^^^^^^^^^^^^
...
146 |     pub checkpoint_id: Option<String>,
    |         ^^^^^^^^^^^^^
147 |     pub base_sha: Option<String>,
    |         ^^^^^^^^
...
151 |     pub completed_at: String,
    |         ^^^^^^^^^^^^
152 |     pub updated_at: String,
    |         ^^^^^^^^^^
    |
    = note: `JournalRow` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
    = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: field `max_parallel` is never read
   --> src/agent/scheduler.rs:174:9
    |
172 | pub struct TaskScheduler {
    |            ------------- field in this struct
173 |     pub pool: SqlitePool,
174 |     pub max_parallel: usize,
    |         ^^^^^^^^^^^^
    |
    = note: `TaskScheduler` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: function `mark_task_started` is never used
   --> src/storage/tasks.rs:209:14
    |
209 | pub async fn mark_task_started(pool: &SqlitePool, id: &str) -> Result<()> {
    |              ^^^^^^^^^^^^^^^^^

warning: `codefactory` (lib test) generated 3 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 57.70s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-12c14b67570e8321)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-tb21-v1492/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-124850
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-tb21-v1492/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-124850
provider_bridge_imported run=dd54bc63-0f54-4243-ba0e-a1f5d81b9562 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=18 mean_reward=0.333
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("model-provider")
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/extract-elf reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/protein-assembly reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/qemu-startup reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sparql-university reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("model-provider")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 368 filtered out; finished in 5522.76s


Evidence report: /Users/leo/Projects/CodeFactory-tb21-v1492/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T14-20-53Z.md

```
