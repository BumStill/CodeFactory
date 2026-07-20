# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-20T16-36-52Z`
- subset: `terminal-bench-21-regression-subset-v1-canary`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `6`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `<disabled>`
- heavy_verifier_timeout_overrides: `<none>`
- heavy_verifier_timeout_multiplier: `<none>`
- verifier_uv_torch_backend: `<none>`
- partial_import_diagnostic: `enabled`

## Agent Binary Preflight

- agent binary source: built from current source (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri/target/debug/codefactory-agent-headless)
- agent binary sha256: 05c1f56139d817738ad14275214a848590ae4b33322c201bbd3acde15ebbbad0

## Preview

- model: `deepseek-v4-pro`
- task_limit: `6`
- concurrency: `4`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-161003`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-161003`

## Agent Usage

- trials_with_metadata: `6`
- model_requests: `306`
- prompt_tokens: `2395270`
- completion_tokens: `140818`
- total_tokens: `2536088`
- tool_calls: `289`

## Result

- run: `97e6b840-cc5f-4bd6-8cac-c6598dbd2d3c`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `6`
- pass_count: `1`
- mean_reward: `0.167`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `0` | `Some("verification")` |
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("environment")` |
| `terminal-bench/configure-git-webserver` | `1` | `None` |
| `terminal-bench/qemu-startup` | `0` | `Some("verification")` |
| `terminal-bench/sanitize-git-repo` | `0` | `Some("verification")` |
| `terminal-bench/write-compressor` | `0` | `Some("verification")` |

## Verifier Environment Warnings

These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.

| Trial | Category | Evidence |
| --- | --- | --- |
| `configure-git-webserver__ndjPVcz` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `qemu-startup__waaPWDm` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sanitize-git-repo__8E673Ea` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `write-compressor__2TaEBPb` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |

## Output Tail

```text
tor v0.8.0
   Compiling brotli v8.0.2
   Compiling derive_more-impl v2.1.1
   Compiling yoke v0.8.2
   Compiling cssparser-macros v0.6.1
   Compiling toml_datetime v1.1.1+spec-1.1.0
   Compiling selectors v0.36.1
   Compiling indexmap v1.9.3
   Compiling aho-corasick v1.1.4
   Compiling glob v0.3.3
   Compiling camino v1.2.2
   Compiling serde_with_macros v3.20.0
   Compiling regex-automata v0.4.14
   Compiling markup5ever v0.38.0
   Compiling cssparser v0.36.0
   Compiling toml v1.1.2+spec-1.1.0
   Compiling derive_more v2.1.1
   Compiling swift-rs v1.0.7
   Compiling serde_derive_internals v0.29.1
   Compiling servo_arc v0.4.3
   Compiling hashbrown v0.12.3
   Compiling rustc-hash v2.1.2
   Compiling schemars v0.8.22
   Compiling bit-vec v0.8.0
   Compiling zerovec v0.11.6
   Compiling zerotrie v0.2.4
   Compiling schemars_derive v0.8.22
   Compiling bit-set v0.8.0
   Compiling jsonptr v0.6.3
   Compiling cargo-platform v0.1.9
   Compiling regex v1.12.3
   Compiling html5ever v0.38.0
   Compiling cfb v0.7.3
   Compiling signal-hook-registry v1.4.8
   Compiling time v0.3.47
   Compiling dyn-clone v1.0.20
   Compiling foldhash v0.2.0
   Compiling base64 v0.21.7
   Compiling infer v0.19.0
   Compiling serde-untagged v0.1.9
   Compiling cargo_metadata v0.19.2
   Compiling json-patch v3.0.1
   Compiling plist v1.9.0
   Compiling serde_with v3.20.0
   Compiling dom_query v0.27.0
   Compiling objc2-exception-helper v0.1.1
   Compiling option-ext v0.2.0
   Compiling tokio-macros v2.7.0
   Compiling mio v1.2.0
   Compiling socket2 v0.6.3
   Compiling objc2 v0.6.4
   Compiling tokio v1.52.3
   Compiling objc2-encode v4.1.0
   Compiling simd-adler32 v0.3.9
   Compiling crc32fast v1.5.0
   Compiling ring v0.17.14
   Compiling tinystr v0.8.3
   Compiling potential_utf v0.1.5
   Compiling icu_locale_core v2.2.0
   Compiling icu_collections v2.2.0
   Compiling heck v0.5.0
   Compiling futures-io v0.3.32
   Compiling adler2 v2.0.1
   Compiling icu_provider v2.2.0
   Compiling icu_normalizer v2.2.0
   Compiling icu_properties v2.2.0
   Compiling crossbeam-utils v0.8.21
   Compiling futures-task v0.3.32
   Compiling idna_adapter v1.2.2
   Compiling idna v1.1.0
   Compiling url v2.5.8
   Compiling urlpattern v0.3.0
   Compiling miniz_oxide v0.8.9
   Compiling tracing-attributes v0.1.31
   Compiling rustc_version v0.4.1
   Compiling toml_datetime v0.7.5+spec-1.1.0
   Compiling core-foundation v0.10.1
   Compiling winnow v0.7.15
   Compiling untrusted v0.9.0
   Compiling embed-resource v3.0.9
   Compiling dirs-sys v0.5.0
   Compiling dirs v6.0.0
   Compiling tauri-winres v0.3.6
   Compiling pkg-config v0.3.33
   Compiling rustls v0.23.40
   Compiling cpufeatures v0.2.17
   Compiling toml v0.9.12+spec-1.1.0
   Compiling tracing-core v0.1.36
   Compiling vcpkg v0.2.15
   Compiling cargo_toml v0.22.3
   Compiling subtle v2.6.1
   Compiling sha2 v0.10.9
   Compiling futures-macro v0.3.32
   Compiling time-macros v0.2.27
   Compiling zlib-rs v0.6.3
   Compiling futures-util v0.3.32
   Compiling tracing v0.1.44
   Compiling tauri-utils v2.9.1
   Compiling fdeflate v0.3.7
   Compiling num-traits v0.2.19
   Compiling bitflags v1.3.2
   Compiling raw-window-handle v0.6.2
   Compiling dpi v0.1.2
   Compiling cookie v0.18.1
   Compiling block2 v0.6.2
   Compiling objc2-core-foundation v0.3.2
   Compiling getrandom v0.2.17
   Compiling objc2-foundation v0.3.2
   Compiling flate2 v1.1.9
   Compiling foreign-types-macros v0.2.3
   Compiling foreign-types-shared v0.3.1
   Compiling foreign-types v0.5.0
   Compiling rustls-webpki v0.103.13
   Compiling png v0.17.16
   Compiling dispatch2 v0.3.1
   Compiling crossbeam-channel v0.5.15
   Compiling core-graphics-types v0.2.0
   Compiling futures-channel v0.3.32
   Compiling security-framework-sys v2.17.0
   Compiling iana-time-zone v0.1.65
   Compiling wry v0.55.1
   Compiling foldhash v0.1.5
   Compiling allocator-api2 v0.2.21
   Compiling parking v2.2.1
   Compiling futures-sink v0.3.32
   Compiling tauri-runtime v2.11.1
   Compiling security-framework v3.7.0
   Compiling core-graphics v0.25.0
   Compiling hashbrown v0.15.5
   Compiling ico v0.5.0
   Compiling tauri-plugin v2.6.1
   Compiling tauri-build v2.6.1
   Compiling rustix v1.1.4
   Compiling tokio-util v0.7.18
   Compiling webpki-roots v1.0.7
   Compiling crc-catalog v2.5.0
   Compiling unicode-segmentation v1.13.2
   Compiling getrandom v0.3.4
   Compiling tauri-runtime-wry v2.11.1
   Compiling h2 v0.4.14
   Compiling keyboard-types v0.7.0
   Compiling crc v3.4.0
   Compiling webpki-roots v0.26.11
   Compiling tauri-codegen v2.6.1
   Compiling hashlink v0.10.0
   Compiling system-configuration-sys v0.6.0
   Compiling png v0.18.1
   Compiling libsqlite3-sys v0.30.1
   Compiling serialize-to-javascript-impl v0.1.2
   Compiling core-foundation v0.9.4
   Compiling spin v0.9.8
   Compiling slab v0.4.12
   Compiling system-configuration v0.7.0
   Compiling serialize-to-javascript v0.1.2
   Compiling hyper v1.9.0
   Compiling concurrent-queue v2.5.0
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
   Compiling tauri v2.11.1
   Compiling tauri-macros v2.6.1
   Compiling tauri-plugin-fs v2.5.1
   Compiling bumpalo v3.20.2
   Compiling signal-hook v0.3.18
   Compiling tower v0.5.3
   Compiling zopfli v0.8.3
   Compiling objc2-app-kit v0.3.2
   Compiling futures-executor v0.3.32
   Compiling flume v0.11.1
   Compiling serde_urlencoded v0.7.1
   Compiling memoffset v0.6.5
   Compiling zip v2.4.2
   Compiling sqlx-core v0.8.6
   Compiling tower-http v0.6.10
   Compiling native-tls v0.2.18
   Compiling tauri-plugin-dialog v2.7.1
   Compiling tauri-plugin-process v2.3.1
   Compiling tauri-plugin-updater v2.10.1
   Compiling tauri-plugin-shell v2.3.5
   Compiling tokio-rustls v0.26.4
   Compiling sqlx-sqlite v0.8.6
   Compiling libgit2-sys v0.17.0+1.8.1
   Compiling termios v0.2.2
   Compiling os_pipe v1.2.3
   Compiling serial-core v0.4.0
   Compiling ioctl-rs v0.1.6
   Compiling rfd v0.16.0
   Compiling dotenvy v0.15.7
   Compiling hex v0.4.3
   Compiling serial-unix v0.4.0
   Compiling sigchld v0.2.4
   Compiling hyper-rustls v0.27.9
   Compiling objc2-web-kit v0.3.2
   Compiling tao v0.35.2
   Compiling muda v0.19.1
   Compiling window-vibrancy v0.6.0
   Compiling objc2-osa-kit v0.3.2
   Compiling sqlx-macros-core v0.8.6
   Compiling tokio-native-tls v0.3.1
   Compiling rustls-platform-verifier v0.7.0
   Compiling xattr v1.6.1
   Compiling filetime v0.2.29
   Compiling pathdiff v0.2.3
   Compiling lazy_static v1.5.0
   Compiling pin-utils v0.1.0
   Compiling sharded-slab v0.1.7
   Compiling nix v0.25.1
   Compiling open v5.3.5
   Compiling tar v0.4.46
   Compiling reqwest v0.13.3
   Compiling hyper-tls v0.6.0
   Compiling osakit v0.3.1
   Compiling sqlx-macros v0.8.6
   Compiling shared_child v1.1.1
   Compiling serial v0.4.0
   Compiling codepage v0.1.2
   Compiling quick-xml v0.31.0
   Compiling codefactory v1.51.1 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri)
   Compiling matchers v0.2.0
   Compiling tracing-log v0.2.0
   Compiling dirs-sys v0.4.1
   Compiling filedescriptor v0.8.3
   Compiling bstr v1.12.1
   Compiling thread_local v1.1.9
   Compiling hashbrown v0.14.5
   Compiling nu-ansi-term v0.50.3
   Compiling downcast-rs v1.2.1
   Compiling minisign-verify v0.2.5
   Compiling shell-words v1.1.1
   Compiling globset v0.4.18
   Compiling portable-pty v0.8.1
   Compiling tracing-subscriber v0.3.23
   Compiling dashmap v6.2.1
   Compiling sqlx v0.8.6
   Compiling dirs v5.0.1
   Compiling calamine v0.26.1
   Compiling reqwest v0.12.28
   Compiling rust_xlsxwriter v0.79.4
   Compiling codefactory-agent-core v0.1.0 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri/crates/agent-core)
   Compiling zip v4.6.1
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

warning: method `is_empty` is never used
  --> src/storage/tasks.rs:31:12
   |
30 | impl TaskConnectorContext {
   | ------------------------- method in this implementation
31 |     pub fn is_empty(&self) -> bool {
   |            ^^^^^^^^

warning: function `mark_task_started` is never used
   --> src/storage/tasks.rs:209:14
    |
209 | pub async fn mark_task_started(pool: &SqlitePool, id: &str) -> Result<()> {
    |              ^^^^^^^^^^^^^^^^^

warning: `codefactory` (lib test) generated 4 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 46.56s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-cf76f6d44f15ee88)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=6 concurrency=4 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-161003
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-161003
provider_bridge_imported run=97e6b840-cc5f-4bd6-8cac-c6598dbd2d3c dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=6 mean_reward=0.167
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("environment")
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/qemu-startup reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 380 filtered out; finished in 1608.95s


```
