# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-10T07-41-20Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `1200`
- heavy_verifier_timeout_overrides: `torch-tensor-parallelism:2400`
- heavy_verifier_timeout_multiplier: `3`
- verifier_uv_torch_backend: `cpu`
- partial_import_diagnostic: `enabled`

## Comparability Notes

- runner-level trial hard timeout watchdog was enabled
- watchdog stopped one or more stale trial containers

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `2`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-agent-eval-next-loop/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260710-062829`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-agent-eval-next-loop/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260710-062829`

## Result

- run: `3f86d0e1-e7a9-465e-9deb-034ee38d4d1a`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `18`
- pass_count: `12`
- mean_reward: `0.667`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `0` | `Some("policy")` |
| `terminal-bench/caffe-cifar-10` | `0` | `Some("verification")` |
| `terminal-bench/circuit-fibsqrt` | `1` | `None` |
| `terminal-bench/configure-git-webserver` | `0` | `Some("verification")` |
| `terminal-bench/count-dataset-tokens` | `1` | `None` |
| `terminal-bench/extract-elf` | `1` | `None` |
| `terminal-bench/filter-js-from-html` | `0` | `Some("environment")` |
| `terminal-bench/install-windows-3.11` | `0` | `Some("verification")` |
| `terminal-bench/kv-store-grpc` | `1` | `None` |
| `terminal-bench/mteb-retrieve` | `1` | `None` |
| `terminal-bench/nginx-request-logging` | `1` | `None` |
| `terminal-bench/protein-assembly` | `1` | `None` |
| `terminal-bench/qemu-startup` | `1` | `None` |
| `terminal-bench/query-optimize` | `0` | `Some("environment")` |
| `terminal-bench/sanitize-git-repo` | `1` | `None` |
| `terminal-bench/sparql-university` | `1` | `None` |
| `terminal-bench/torch-tensor-parallelism` | `1` | `None` |
| `terminal-bench/write-compressor` | `1` | `None` |

## Watchdog Interventions

The regression runner stopped stale trial containers so the remaining matrix could finish.

| Trial | Elapsed sec | Action | Containers |
| --- | ---: | --- | --- |
| `filter-js-from-html__L7TqtAo` | `1200` | `docker-stop` | `filter-js-from-html__l7tqtao-main-1` |
| `query-optimize__wvkA7Mt` | `1200` | `docker-stop` | `query-optimize__wvka7mt-main-1` |

## Verifier Environment Warnings

These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.

| Trial | Category | Evidence |
| --- | --- | --- |
| `caffe-cifar-10__2dfkoq9` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `circuit-fibsqrt__6LJFwTr` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `configure-git-webserver__GatSkXT` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `count-dataset-tokens__npSJhvf` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `extract-elf__yagSFgE` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `filter-js-from-html__L7TqtAo` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `install-windows-3.11__zvraCmZ` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `nginx-request-logging__jqmHJxg` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `protein-assembly__SuAKKrG` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `qemu-startup__QB4ABt5` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `query-optimize__wvkA7Mt` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sanitize-git-repo__tWHxwRw` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sparql-university__ETEUSRB` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `torch-tensor-parallelism__rya6ZfP` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `write-compressor__oN9PpYW` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |

## Output Tail

```text
k-xml v0.39.4
   Compiling string_cache v0.9.0
   Compiling brotli-decompressor v5.0.0
   Compiling semver v1.0.28
   Compiling utf-8 v0.7.6
   Compiling ctor-proc-macro v0.0.7
   Compiling dunce v1.0.5
   Compiling same-file v1.0.6
   Compiling dtoa v1.0.11
   Compiling version_check v0.9.5
   Compiling dtoa-short v0.3.5
   Compiling ctor v0.8.0
   Compiling walkdir v2.5.0
   Compiling yoke v0.8.2
   Compiling tendril v0.5.0
   Compiling brotli v8.0.2
   Compiling serde_with_macros v3.20.0
   Compiling cssparser-macros v0.6.1
   Compiling derive_more-impl v2.1.1
   Compiling selectors v0.36.1
   Compiling toml_datetime v1.1.1+spec-1.1.0
   Compiling indexmap v1.9.3
   Compiling aho-corasick v1.1.4
   Compiling glob v0.3.3
   Compiling camino v1.2.2
   Compiling markup5ever v0.38.0
   Compiling regex-automata v0.4.14
   Compiling derive_more v2.1.1
   Compiling swift-rs v1.0.7
   Compiling toml v1.1.2+spec-1.1.0
   Compiling cssparser v0.36.0
   Compiling serde_derive_internals v0.29.1
   Compiling servo_arc v0.4.3
   Compiling rustc-hash v2.1.2
   Compiling hashbrown v0.12.3
   Compiling bit-vec v0.8.0
   Compiling schemars v0.8.22
   Compiling zerovec v0.11.6
   Compiling zerotrie v0.2.4
   Compiling bit-set v0.8.0
   Compiling schemars_derive v0.8.22
   Compiling regex v1.12.3
   Compiling cargo-platform v0.1.9
   Compiling jsonptr v0.6.3
   Compiling html5ever v0.38.0
   Compiling cfb v0.7.3
   Compiling signal-hook-registry v1.4.8
   Compiling time v0.3.47
   Compiling foldhash v0.2.0
   Compiling base64 v0.21.7
   Compiling dyn-clone v1.0.20
   Compiling infer v0.19.0
   Compiling serde-untagged v0.1.9
   Compiling json-patch v3.0.1
   Compiling cargo_metadata v0.19.2
   Compiling plist v1.9.0
   Compiling serde_with v3.20.0
   Compiling objc2-exception-helper v0.1.1
   Compiling option-ext v0.2.0
   Compiling tokio-macros v2.7.0
   Compiling socket2 v0.6.3
   Compiling dom_query v0.27.0
   Compiling mio v1.2.0
   Compiling objc2 v0.6.4
   Compiling generic-array v0.14.7
   Compiling objc2-encode v4.1.0
   Compiling tokio v1.52.3
   Compiling typenum v1.20.0
   Compiling tinystr v0.8.3
   Compiling potential_utf v0.1.5
   Compiling icu_locale_core v2.2.0
   Compiling icu_collections v2.2.0
   Compiling simd-adler32 v0.3.9
   Compiling crc32fast v1.5.0
   Compiling ring v0.17.14
   Compiling heck v0.5.0
   Compiling icu_provider v2.2.0
   Compiling icu_properties v2.2.0
   Compiling icu_normalizer v2.2.0
   Compiling zeroize v1.8.2
   Compiling rustls-pki-types v1.14.1
   Compiling futures-task v0.3.32
   Compiling idna_adapter v1.2.2
   Compiling idna v1.1.0
   Compiling futures-io v0.3.32
   Compiling url v2.5.8
   Compiling urlpattern v0.3.0
   Compiling crossbeam-utils v0.8.21
   Compiling adler2 v2.0.1
   Compiling miniz_oxide v0.8.9
   Compiling crypto-common v0.1.7
   Compiling block-buffer v0.10.4
   Compiling rustc_version v0.4.1
   Compiling tracing-attributes v0.1.31
   Compiling core-foundation v0.10.1
   Compiling toml_datetime v0.7.5+spec-1.1.0
   Compiling untrusted v0.9.0
   Compiling futures-sink v0.3.32
   Compiling winnow v0.7.15
   Compiling embed-resource v3.0.9
   Compiling digest v0.10.7
   Compiling dirs-sys v0.5.0
   Compiling dirs v6.0.0
   Compiling tauri-winres v0.3.6
   Compiling slab v0.4.12
   Compiling pkg-config v0.3.33
   Compiling rustls v0.23.40
   Compiling tracing-core v0.1.36
   Compiling toml v0.9.12+spec-1.1.0
   Compiling cpufeatures v0.2.17
   Compiling vcpkg v0.2.15
   Compiling subtle v2.6.1
   Compiling cargo_toml v0.22.3
   Compiling sha2 v0.10.9
   Compiling futures-macro v0.3.32
   Compiling time-macros v0.2.27
   Compiling zlib-rs v0.6.3
   Compiling tauri-utils v2.9.1
   Compiling futures-util v0.3.32
   Compiling block2 v0.6.2
   Compiling objc2-core-foundation v0.3.2
   Compiling tracing v0.1.44
   Compiling fdeflate v0.3.7
   Compiling objc2-foundation v0.3.2
   Compiling num-traits v0.2.19
   Compiling raw-window-handle v0.6.2
   Compiling bitflags v1.3.2
   Compiling dpi v0.1.2
   Compiling cookie v0.18.1
   Compiling getrandom v0.2.17
   Compiling flate2 v1.1.9
   Compiling foreign-types-macros v0.2.3
   Compiling foreign-types-shared v0.3.1
   Compiling rustix v1.1.4
   Compiling foreign-types v0.5.0
   Compiling png v0.17.16
   Compiling rustls-webpki v0.103.13
   Compiling dispatch2 v0.3.1
   Compiling crossbeam-channel v0.5.15
   Compiling futures-channel v0.3.32
   Compiling core-graphics-types v0.2.0
   Compiling security-framework-sys v2.17.0
   Compiling iana-time-zone v0.1.65
   Compiling http-body v1.0.1
   Compiling system-configuration-sys v0.6.0
   Compiling wry v0.55.1
   Compiling parking v2.2.1
   Compiling httparse v1.10.1
   Compiling allocator-api2 v0.2.21
   Compiling tauri-runtime v2.11.1
   Compiling foldhash v0.1.5
   Compiling security-framework v3.7.0
   Compiling hashbrown v0.15.5
   Compiling tauri-plugin v2.6.1
   Compiling tauri-build v2.6.1
   Compiling core-graphics v0.25.0
   Compiling ico v0.5.0
   Compiling tokio-util v0.7.18
   Compiling webpki-roots v1.0.7
   Compiling unicode-segmentation v1.13.2
   Compiling tower-service v0.3.3
   Compiling try-lock v0.2.5
   Compiling getrandom v0.3.4
   Compiling crc-catalog v2.5.0
   Compiling ryu v1.0.23
   Compiling atomic-waker v1.1.2
   Compiling tauri-runtime-wry v2.11.1
   Compiling keyboard-types v0.7.0
   Compiling crc v3.4.0
   Compiling h2 v0.4.14
   Compiling want v0.3.1
   Compiling webpki-roots v0.26.11
   Compiling tauri-codegen v2.6.1
   Compiling hashlink v0.10.0
   Compiling png v0.18.1
   Compiling libsqlite3-sys v0.30.1
   Compiling serialize-to-javascript-impl v0.1.2
   Compiling core-foundation v0.9.4
   Compiling spin v0.9.8
   Compiling mime v0.3.17
   Compiling system-configuration v0.7.0
   Compiling serialize-to-javascript v0.1.2
   Compiling concurrent-queue v2.5.0
   Compiling serde_repr v0.1.20
   Compiling hyper v1.9.0
   Compiling ipnet v2.12.0
   Compiling embed_plist v1.2.2
   Compiling hyper-util v0.1.20
   Compiling tempfile v3.27.0
   Compiling tauri v2.11.1
   Compiling tauri-macros v2.6.1
   Compiling tauri-plugin-fs v2.5.1
   Compiling event-listener v5.4.1
   Compiling tokio-stream v0.1.18
   Compiling objc2-app-kit v0.3.2
   Compiling crossbeam-queue v0.3.12
   Compiling chrono v0.4.44
   Compiling atoi v2.0.0
   Compiling libz-sys v1.1.28
   Compiling either v1.15.0
   Compiling futures-intrusive v0.5.0
   Compiling sync_wrapper v1.0.2
   Compiling encoding_rs v0.8.35
   Compiling tower-layer v0.3.3
   Compiling native-tls v0.2.18
   Compiling signal-hook v0.3.18
   Compiling bumpalo v3.20.2
   Compiling zopfli v0.8.3
   Compiling tower v0.5.3
   Compiling futures-executor v0.3.32
   Compiling flume v0.11.1
   Compiling serde_urlencoded v0.7.1
   Compiling http-body-util v0.1.3
   Compiling memoffset v0.6.5
   Compiling zip v2.4.2
   Compiling sqlx-core v0.8.6
   Compiling tower-http v0.6.10
   Compiling tauri-plugin-updater v2.10.1
   Compiling tauri-plugin-process v2.3.1
   Compiling tauri-plugin-dialog v2.7.1
   Compiling tauri-plugin-shell v2.3.5
   Compiling tokio-rustls v0.26.4
   Compiling libgit2-sys v0.17.0+1.8.1
   Compiling termios v0.2.2
   Compiling os_pipe v1.2.3
   Compiling serial-core v0.4.0
   Compiling ioctl-rs v0.1.6
   Compiling dotenvy v0.15.7
   Compiling rfd v0.16.0
   Compiling hex v0.4.3
   Compiling serial-unix v0.4.0
   Compiling sigchld v0.2.4
   Compiling hyper-rustls v0.27.9
   Compiling objc2-web-kit v0.3.2
   Compiling tao v0.35.2
   Compiling window-vibrancy v0.6.0
   Compiling muda v0.19.1
   Compiling sqlx-sqlite v0.8.6
   Compiling objc2-osa-kit v0.3.2
   Compiling tokio-native-tls v0.3.1
   Compiling rustls-platform-verifier v0.7.0
   Compiling xattr v1.6.1
   Compiling filetime v0.2.29
   Compiling pin-utils v0.1.0
   Compiling sqlx-macros-core v0.8.6
   Compiling lazy_static v1.5.0
   Compiling pathdiff v0.2.3
   Compiling sharded-slab v0.1.7
   Compiling tar v0.4.46
   Compiling open v5.3.5
   Compiling nix v0.25.1
   Compiling reqwest v0.13.3
   Compiling hyper-tls v0.6.0
   Compiling osakit v0.3.1
   Compiling shared_child v1.1.1
   Compiling serial v0.4.0
   Compiling quick-xml v0.31.0
   Compiling codepage v0.1.2
   Compiling codefactory v1.42.7 (/Users/leo/Projects/CodeFactory-agent-eval-next-loop/src-tauri)
   Compiling matchers v0.2.0
   Compiling tracing-log v0.2.0
   Compiling sqlx-macros v0.8.6
   Compiling dirs-sys v0.4.1
   Compiling filedescriptor v0.8.3
   Compiling bstr v1.12.1
   Compiling thread_local v1.1.9
   Compiling nu-ansi-term v0.50.3
   Compiling minisign-verify v0.2.5
   Compiling shell-words v1.1.1
   Compiling downcast-rs v1.2.1
   Compiling hashbrown v0.14.5
   Compiling globset v0.4.18
   Compiling tracing-subscriber v0.3.23
   Compiling portable-pty v0.8.1
   Compiling dashmap v6.2.1
   Compiling sqlx v0.8.6
   Compiling dirs v5.0.1
   Compiling calamine v0.26.1
   Compiling reqwest v0.12.28
   Compiling rust_xlsxwriter v0.79.4
   Compiling zip v4.6.1
   Compiling keyring v3.6.3
   Compiling git2 v0.19.0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 54.89s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7f556f249834bf98)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=2 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-agent-eval-next-loop/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260710-062829
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-agent-eval-next-loop/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260710-062829
provider_bridge_imported run=3f86d0e1-e7a9-465e-9deb-034ee38d4d1a dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=18 mean_reward=0.667
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/extract-elf reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=0 failure_class=Some("environment")
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/protein-assembly reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/qemu-startup reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("environment")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/sparql-university reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/write-compressor reward=1 failure_class=None
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 193 filtered out; finished in 4370.53s


```
