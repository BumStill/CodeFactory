use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use url::Url;

pub const EXECUTION_CONTRACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../agent_contracts/execution_completion.md"
));

pub fn execution_contract_sha256() -> String {
    let digest = Sha256::digest(EXECUTION_CONTRACT.as_bytes());
    format!("{digest:x}")
}

pub fn build_system_prompt(allow_external_network: bool) -> String {
    build_scoped_system_prompt(allow_external_network, true)
}

pub fn build_product_system_prompt(allow_external_network: bool) -> String {
    format!(
        "{}\n\nThe selected project is the writable workspace. Use $TMPDIR for temporary files; writes outside the workspace and $TMPDIR are denied.",
        build_scoped_system_prompt(allow_external_network, false)
    )
}

fn build_scoped_system_prompt(
    allow_external_network: bool,
    include_benchmark_boundaries: bool,
) -> String {
    let network_capability = if allow_external_network {
        "External network access is available under the active environment policy. Use it only \
for task-required dependency or source retrieval; the environment remains the enforcement point."
    } else {
        "Network access is denied by default except for loopback functional probes."
    };
    let benchmark_boundary = if include_benchmark_boundaries {
        "Never inspect hidden verifiers or benchmark solutions. "
    } else {
        ""
    };
    format!(
        "You are CodeFactory's autonomous coding agent. Work only through the provided tools. \
{benchmark_boundary}Never inspect secret stores. {network_capability} Do not \
claim completion until the shared completion gate accepts structured execution evidence.\n\n{}",
        EXECUTION_CONTRACT.trim()
    )
}

pub fn build_completion_recovery_prompt(evidence: &CompletionEvidence) -> String {
    format!(
        "The completion gate rejected the attempted final response. Continue using tools until \
the following evidence blockers are resolved: {}. Run only what resolves these blockers and \
do not restate your previous summary — the user has already seen it. Once the blockers are \
resolved, reply in the user's language with a brief final answer that adds only the new \
verification outcome, and never mention internal mechanisms such as this gate.",
        evidence.blockers.join("; ")
    )
}

pub fn build_completion_ready_prompt() -> &'static str {
    "The structured completion evidence is satisfied as a candidate, but final acceptance still \
requires one coverage audit against the original request. Map every explicitly named behavior, \
component, artifact, and environment constraint to concrete evidence from this run. Import, file \
existence, or compilation alone does not prove named component behavior. If any required behavior \
lacks a functional check, use the available tools now to run the missing check and repair any \
failure. For source compatibility work, rerun a repository-wide residual search after the last \
source edit, cover every generated or compiled source suffix found in the build configuration, and \
make the command succeed only when no unresolved matches remain. If coverage is complete, stop and \
return the final response as a concise user-facing summary in the user's language with the \
verification evidence; do not repeat analysis or summaries the user has already seen — reference \
them and add only what is new. Never mention internal mechanisms (completion gate, coverage \
audit, candidate delivery) in the user-facing text; do not emit tool protocol markup, commands, \
or XML."
}

pub fn should_prompt_budget_convergence(remaining_model_rounds: u32) -> bool {
    (1..=16).contains(&remaining_model_rounds)
}

pub fn should_prompt_time_convergence(remaining_wall_sec: u64, total_wall_sec: u64) -> bool {
    total_wall_sec > 0
        && remaining_wall_sec > 0
        && remaining_wall_sec <= total_wall_sec.saturating_mul(2) / 3
}

pub fn build_budget_convergence_prompt(
    remaining_model_rounds: u32,
    evidence: &CompletionEvidence,
) -> String {
    build_convergence_prompt(
        &format!("Only {remaining_model_rounds} model rounds remain."),
        evidence,
    )
}

pub fn build_time_convergence_prompt(
    remaining_wall_sec: u64,
    evidence: &CompletionEvidence,
) -> String {
    build_convergence_prompt(
        &format!("Only {remaining_wall_sec} seconds of execution time remain."),
        evidence,
    )
}

fn build_convergence_prompt(window: &str, evidence: &CompletionEvidence) -> String {
    let blockers = if evidence.blockers.is_empty() {
        "none recorded".to_owned()
    } else {
        evidence.blockers.join("; ")
    };
    format!(
        "{window} Re-read the original request, identify \
every explicitly required delivery stage that is still missing, and prioritize the smallest \
end-to-end path that satisfies it. For source or package work, \
finish build, install, run outside the source directory, named component behavior checks, and \
focused project tests in that order; inspect the build configuration plus generated and compiled \
source inputs rather than scanning only one familiar file type. For compatibility migrations, \
derive local import aliases from the source and scan every discovered alias with token-safe, \
idempotent replacements. Start from each exact failing API member reported by build, runtime, or \
tests, then add candidate spellings only when repository references or a language adapter support \
them. Write matches to a temporary results file, preserve the `grep` or `rg` status, reject status greater than 1, \
and finish with `test ! -s`; do not mask search errors or let the normal no-match status turn a \
clean scan into a failed command. Then rerun \
the same issue scan and require no unresolved matches before finalizing. Once the current source revision installs, do not \
keep installing speculative dependencies before the external runtime or focused tests expose a \
concrete missing dependency. For a background service, finish \
PID, logs, bounded readiness, and a real client probe. \
When a legacy project test fails at a newly installed dependency API, inspect the declared \
constraints and try a compatible dependency version before adding speculative compatibility shims \
to project source; after either repair, rerun the original failing test immediately. \
Every remaining tool call must directly resolve a current completion blocker. Do not investigate \
unrelated test failures, optional compatibility concerns, or hidden tests while a required delivery \
stage is missing. To reduce model round trips, batch related reads, edits, and checks into one \
bounded tool call when their order and failure handling remain clear. If all blockers are resolved, \
return the final response now. Current completion \
blockers: {blockers}."
    )
}

pub fn sanitize_completion_summary(text: &str) -> String {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    let contains_tool_protocol = lower.contains("tool_calls")
        || lower.contains("<tool_call")
        || lower.contains("invoke name=\"run_shell\"")
        || trimmed.contains("DSML｜｜");
    if trimmed.is_empty() || contains_tool_protocol {
        "Implementation completed and post-change verification passed.".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny { rule: String, reason: String },
}

impl PolicyDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone)]
pub struct BenchmarkPolicy {
    product: ProductPolicy,
}

#[derive(Debug, Clone)]
pub struct ProductPolicy {
    allow_loopback_network: bool,
    allow_external_network: bool,
}

impl Default for BenchmarkPolicy {
    fn default() -> Self {
        Self {
            product: ProductPolicy::default(),
        }
    }
}

impl Default for ProductPolicy {
    fn default() -> Self {
        Self {
            allow_loopback_network: true,
            allow_external_network: false,
        }
    }
}

impl ProductPolicy {
    pub fn new(allow_network: bool) -> Self {
        Self {
            allow_loopback_network: true,
            allow_external_network: allow_network,
        }
    }

    pub fn deny_all_network() -> Self {
        Self {
            allow_loopback_network: false,
            allow_external_network: false,
        }
    }

    pub fn with_network_access(allow_external_network: bool) -> Self {
        Self::new(allow_external_network)
    }

    pub fn evaluate_command(&self, command: &str) -> PolicyDecision {
        let lower = command.to_ascii_lowercase();
        if contains_any(
            &lower,
            &[
                "/proc/self/environ",
                "/.ssh/",
                "~/.ssh",
                "/.aws/credentials",
                "~/.aws/credentials",
                "/.config/gcloud/credentials",
                "printenv",
                "security find-generic-password",
                "security find-internet-password",
            ],
        ) || lower.trim() == "env"
            || lower.starts_with("env |")
        {
            return deny("secret_access", "access to secret stores is prohibited");
        }

        if let Some(reason) = self.network_denial_reason(command, &lower) {
            return deny("network", reason);
        }

        PolicyDecision::Allow
    }

    fn network_denial_reason(&self, command: &str, lower: &str) -> Option<&'static str> {
        if self.allow_external_network {
            return None;
        }
        let urls = extract_urls(command);
        if !urls.is_empty() {
            let all_loopback = urls.iter().all(is_loopback_url);
            if !self.allow_loopback_network || !all_loopback {
                return Some("non-loopback network access is prohibited");
            }
        }

        let network_client = first_command_word(lower).is_some_and(|word| {
            matches!(
                word,
                "curl"
                    | "wget"
                    | "nc"
                    | "netcat"
                    | "telnet"
                    | "ssh"
                    | "scp"
                    | "sftp"
                    | "ftp"
                    | "grpcurl"
            )
        });
        if network_client && urls.is_empty() {
            let has_loopback = lower.split_whitespace().any(is_loopback_host_token);
            if !self.allow_loopback_network || !has_loopback {
                return Some("network clients may only target loopback addresses");
            }
        }

        let package_or_remote_fetch = contains_any(
            lower,
            &[
                "git clone ",
                "git fetch ",
                "git pull ",
                "apt-get ",
                "apt ",
                "yum ",
                "dnf ",
                "apk add ",
                "brew install ",
                "cargo install ",
                "npm install ",
                "pnpm install ",
                "yarn install ",
                "pip install ",
                "pip3 install ",
            ],
        );
        if package_or_remote_fetch
            && !contains_any(
                lower,
                &[
                    "--offline",
                    "--no-index",
                    "--network=none",
                    "npm install --offline",
                    "pnpm install --offline",
                    "yarn install --offline",
                ],
            )
        {
            return Some("package and source downloads require explicit offline mode");
        }

        None
    }
}

impl BenchmarkPolicy {
    pub fn new(allow_network: bool) -> Self {
        Self {
            product: ProductPolicy::new(allow_network),
        }
    }

    pub fn deny_all_network() -> Self {
        Self {
            product: ProductPolicy::deny_all_network(),
        }
    }

    pub fn with_network_access(allow_external_network: bool) -> Self {
        Self::new(allow_external_network)
    }

    pub fn evaluate_command(&self, command: &str) -> PolicyDecision {
        let lower = command.to_ascii_lowercase();

        if is_root_wide_hidden_artifact_discovery(&lower)
            || contains_any(
                &lower,
                &[
                    "/tests/",
                    "/tests ",
                    "/tests\"",
                    "/tests'",
                    "/tests/test.sh",
                    "hidden_verifier",
                    "hidden-verifier",
                    "/grader/",
                ],
            )
        {
            return deny(
                "hidden_verifier",
                "access to hidden verifier or grader paths is prohibited",
            );
        }

        if contains_any(
            &lower,
            &[
                "/solution/",
                "/solution.",
                "/solution ",
                "solution.sh",
                "answer.sh",
                "/answers/",
            ],
        ) {
            return deny(
                "benchmark_solution",
                "access to benchmark solution artifacts is prohibited",
            );
        }

        self.product.evaluate_command(command)
    }
}

fn is_root_wide_hidden_artifact_discovery(command: &str) -> bool {
    let root_wide_find = command
        .split([';', '\n', '|'])
        .map(str::trim)
        .any(|segment| segment.starts_with("find / ") || segment == "find /");
    root_wide_find
        && contains_any(
            command,
            &[
                "test*",
                "verify*",
                "verifier*",
                "check*",
                "grader*",
                "solution*",
                "answer*",
            ],
        )
}

fn deny(rule: &str, reason: &str) -> PolicyDecision {
    PolicyDecision::Deny {
        rule: rule.to_owned(),
        reason: reason.to_owned(),
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn first_command_word(command: &str) -> Option<&str> {
    command
        .split(|character: char| character.is_whitespace() || character == ';' || character == '|')
        .find(|part| !part.is_empty() && !part.contains('='))
}

fn extract_urls(command: &str) -> Vec<Url> {
    command
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|character: char| {
                matches!(character, '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';')
            });
            if candidate.starts_with("http://") || candidate.starts_with("https://") {
                Url::parse(candidate).ok()
            } else {
                None
            }
        })
        .collect()
}

fn is_loopback_url(url: &Url) -> bool {
    url.host_str().is_some_and(is_loopback_host)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "::1"
        || host == "[::1]"
}

fn is_loopback_host_token(token: &str) -> bool {
    let token = token.trim_matches(|character: char| {
        matches!(character, '\'' | '"' | '[' | ']' | '(' | ')' | ',' | ';')
    });
    is_loopback_host(token) || token.starts_with("localhost:") || token.starts_with("127.0.0.1:")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKind {
    ReadOnly,
    Mutation,
    Verification,
    RuntimeProbe,
    BackgroundServiceStart,
    FunctionalProbe { bounded: bool },
}

pub fn effective_command_timeout_sec(command: &str, requested: u64, maximum: u64) -> u64 {
    let maximum = maximum.max(1);
    let requested = requested.clamp(1, maximum);
    let lower = command.to_ascii_lowercase();
    if is_dependency_install_command(&lower)
        || contains_any(
            &lower,
            &[
                "setup.py build",
                "build_ext",
                "cargo build",
                "npm run build",
                "pnpm build",
                "yarn build",
                "bun run build",
                "cmake --build",
                "mvn package",
                "gradle build",
                "gradlew build",
                "go build",
                "docker build",
                "podman build",
                "dotnet build",
                "swift build",
                "xcodebuild",
            ],
        )
    {
        maximum
    } else {
        requested
    }
}

pub fn classify_command(command: &str, timeout_ms: u64) -> ToolKind {
    let lower = command.to_ascii_lowercase();
    if is_functional_probe(&lower) {
        return ToolKind::FunctionalProbe {
            bounded: timeout_ms > 0 && has_command_level_bound(&lower),
        };
    }
    if contains_any(
        &lower,
        &[
            "nohup ",
            "docker run -d",
            "podman run -d",
            "systemctl start ",
        ],
    ) || lower.trim_end().ends_with('&')
    {
        return ToolKind::BackgroundServiceStart;
    }
    if is_dependency_install_command(&lower) {
        return ToolKind::Mutation;
    }
    let single_command_version_check = !contains_any(&lower, &["&&", ";", "\n"])
        && (lower.contains("--version") || command.contains(" -V"))
        && first_command_word(&lower).is_some_and(|word| {
            matches!(
                word,
                "python"
                    | "python3"
                    | "node"
                    | "ruby"
                    | "perl"
                    | "java"
                    | "dotnet"
                    | "pip"
                    | "pip3"
                    | "pytest"
                    | "npm"
                    | "pnpm"
                    | "yarn"
                    | "bun"
                    | "cargo"
                    | "rustc"
                    | "go"
                    | "gcc"
                    | "clang"
                    | "cmake"
            )
        });
    if single_command_version_check {
        return ToolKind::ReadOnly;
    }
    let shell_test_command = lower.split([';', '\n', '&', '|']).any(|segment| {
        let command = segment.trim_start();
        command == "test"
            || command.starts_with("test ")
            || command == "["
            || command.starts_with("[ ")
    });
    if shell_test_command
        || contains_any(
            &lower,
            &[
                "pytest",
                "unittest",
                "vitest",
                "jest",
                "playwright test",
                "tsc --noemit",
                "tsc -p",
                "tsc -b",
                "cargo check",
                "cargo build",
                "cargo test",
                "npm run build",
                "npm run lint",
                "pnpm build",
                "pnpm lint",
                "pnpm test",
                "yarn build",
                "yarn test",
                "make check",
                "make test",
                "ctest",
                "go test",
                "mvn test",
                "gradle test",
            ],
        )
    {
        return ToolKind::Verification;
    }
    // NOTE: bare `printf`/`echo` (no redirect) is how investigation batches
    // print section headers — that is not a mutation. A `printf … > file`
    // still classifies as Mutation via the redirect check below.
    if contains_any(
        &lower,
        &[
            "apply_patch",
            "sed -i",
            "perl -pi",
            "tee ",
            "cat >",
            "cat >>",
            "touch ",
            "mkdir ",
            "rm ",
            "mv ",
            "cp ",
            "install ",
            "git apply",
        ],
    ) || lower.contains(" > ")
        || lower.contains(" >> ")
    {
        return ToolKind::Mutation;
    }
    let inline_interpreter_snippet = contains_any(
        &lower,
        &[
            "python -c ",
            "python3 -c ",
            "node -e ",
            "ruby -e ",
            "perl -e ",
        ],
    );
    if is_external_source_runtime_command(&lower) {
        return ToolKind::RuntimeProbe;
    }
    let runtime_smoke = !lower.contains("<<")
        && !inline_interpreter_snippet
        && first_command_word(&lower).is_some_and(|word| {
            matches!(
                word,
                "node" | "python" | "python3" | "ruby" | "perl" | "java" | "dotnet"
            ) || word.starts_with("./")
        });
    if runtime_smoke {
        return ToolKind::RuntimeProbe;
    }
    ToolKind::ReadOnly
}

fn is_dependency_install_command(command: &str) -> bool {
    contains_any(
        command,
        &[
            "pip install ",
            "pip3 install ",
            "python -m pip install ",
            "python3 -m pip install ",
            "uv pip install ",
            "uv add ",
            "npm install",
            "npm i ",
            "npm add ",
            "pnpm install",
            "pnpm add ",
            "yarn install",
            "yarn add ",
            "bun install",
            "bun add ",
            "cargo install ",
            "gem install ",
            "bundle install",
            "composer install",
            "dotnet add package ",
            "go install ",
            "apt-get install ",
            "apt install ",
            "apk add ",
            "dnf install ",
            "yum install ",
            "brew install ",
        ],
    )
}

fn is_project_test_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "pytest",
            "unittest",
            "vitest",
            "jest",
            "cargo test",
            "npm test",
            "npm run test",
            "pnpm test",
            "yarn test",
            "bun test",
            "make test",
            "ctest",
            "go test",
            "mvn test",
            "gradle test",
            "gradlew test",
            "dotnet test",
            "swift test",
        ],
    )
}

fn is_functional_probe(command: &str) -> bool {
    let loopback =
        command.contains("localhost") || command.contains("127.0.0.1") || command.contains("[::1]");
    let known_client = contains_any(
        command,
        &["curl ", "wget ", "grpcurl ", "nc ", "netcat ", "redis-cli "],
    );
    let inline_protocol_client = contains_any(command, &["python -c ", "python3 -c "])
        && contains_any(
            command,
            &[
                "grpc.",
                "socket.",
                "http.client",
                "requests.",
                "urllib.",
                "redis.",
                "psycopg",
                "pymongo",
                "mysql.connector",
            ],
        );
    loopback && (known_client || inline_protocol_client)
}

fn has_command_level_bound(command: &str) -> bool {
    contains_any(
        command,
        &[
            "timeout ",
            "--max-time",
            "--connect-timeout",
            "--timeout",
            "timeout=",
            "settimeout(",
            "wait_for(",
            " -m ",
            " -w ",
        ],
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub request_id: String,
    pub command: String,
    #[serde(default)]
    pub working_directory: Option<String>,
    pub kind: ToolKind,
    pub sequence: u64,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub return_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub semantic_failure: bool,
}

impl ToolOutcome {
    pub fn succeeded(&self) -> bool {
        self.return_code == Some(0) && self.error.is_none() && !self.semantic_failure
    }

    pub fn with_detected_semantic_failure(mut self) -> Self {
        self.semantic_failure = detect_semantic_failure(&self.stdout, &self.stderr)
            || has_mismatched_checksums(&self.command, &self.stdout, &self.stderr);
        self
    }
}

pub fn detect_semantic_failure(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let explicit_error_line = combined.lines().any(|line| {
        let line = line.trim();
        let reports_error = line.starts_with("error:") || line.contains(" error:");
        let benign_summary = contains_any(
            line,
            &[
                "0 error",
                "no error",
                "without error",
                "error: none",
                "error: null",
                "is_error: false",
            ],
        );
        reports_error && !benign_summary
    });
    explicit_error_line
        || contains_any(
            &combined,
            &[
                "tests failed",
                "test failed",
                "\nfailed ",
                " failed in ",
                " failed, ",
                "\nfailures\n",
                "build failed",
                "command not found",
                "no module named",
                "modulenotfounderror",
                "traceback (most recent call last)",
                "verification failed",
                "failed to fetch",
                "invalid compressed data",
                "unexpected eof in archive",
                "error is not recoverable",
                "internal compiler error",
                "externally-managed-environment",
                "can't open file",
            ],
        )
}

fn has_mismatched_checksums(command: &str, stdout: &str, stderr: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let checksum_invocations = ["md5sum", "sha1sum", "sha256sum", "sha512sum"]
        .iter()
        .map(|program| lower.matches(program).count())
        .sum::<usize>();
    if checksum_invocations < 2 {
        return false;
    }

    let checksums = format!("{stdout}\n{stderr}")
        .split_whitespace()
        .map(|token| token.trim_matches(|character: char| !character.is_ascii_hexdigit()))
        .filter(|token| matches!(token.len(), 32 | 40 | 64 | 128))
        .filter(|token| token.chars().all(|character| character.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    checksums.len() > 1
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionEvidence {
    pub require_action: bool,
    pub outcome_count: u64,
    pub last_mutation_sequence: Option<u64>,
    pub last_successful_verification_sequence: Option<u64>,
    pub last_failure_sequence: Option<u64>,
    pub last_service_start_sequence: Option<u64>,
    pub last_service_pid_evidence_sequence: Option<u64>,
    pub last_service_log_evidence_sequence: Option<u64>,
    pub last_bounded_probe_sequence: Option<u64>,
    pub required_source_scan_extensions: Vec<String>,
    pub source_evidence_paths: Vec<String>,
    pub last_source_scan_sequence: Option<u64>,
    pub source_delivery_required: bool,
    pub project_tests_required: bool,
    pub last_source_mutation_sequence: Option<u64>,
    pub last_source_install_sequence: Option<u64>,
    pub last_external_source_runtime_sequence: Option<u64>,
    pub last_project_test_sequence: Option<u64>,
    pub last_successful_project_test_sequence: Option<u64>,
    pub missing_test_runner: Option<String>,
    pub completed: bool,
    pub blockers: Vec<String>,
}

pub fn evaluate_budget_command(
    remaining_model_rounds: u32,
    evidence: &CompletionEvidence,
    command: &str,
    kind: &ToolKind,
) -> PolicyDecision {
    evaluate_budget_command_in_directory(remaining_model_rounds, evidence, command, kind, None)
}

pub fn evaluate_budget_command_in_directory(
    remaining_model_rounds: u32,
    evidence: &CompletionEvidence,
    command: &str,
    kind: &ToolKind,
    working_directory: Option<&str>,
) -> PolicyDecision {
    evaluate_budget_command_with_time_in_directory(
        remaining_model_rounds,
        None,
        evidence,
        command,
        kind,
        working_directory,
    )
}

pub fn evaluate_budget_command_with_time(
    remaining_model_rounds: u32,
    wall_time: Option<(u64, u64)>,
    evidence: &CompletionEvidence,
    command: &str,
    kind: &ToolKind,
) -> PolicyDecision {
    evaluate_budget_command_with_time_in_directory(
        remaining_model_rounds,
        wall_time,
        evidence,
        command,
        kind,
        None,
    )
}

pub fn evaluate_budget_command_with_time_in_directory(
    remaining_model_rounds: u32,
    wall_time: Option<(u64, u64)>,
    evidence: &CompletionEvidence,
    command: &str,
    kind: &ToolKind,
    working_directory: Option<&str>,
) -> PolicyDecision {
    let time_finalization_window =
        wall_time.is_some_and(|(remaining, total)| total > 0 && remaining <= total / 3);
    let source_delivery_checkpoint = evidence.source_delivery_required
        && wall_time.is_some_and(|(remaining, total)| {
            total > 0 && remaining <= total.saturating_mul(2) / 3
        });
    if evidence.source_delivery_required {
        let delivery_stage_count = [
            is_source_install_command_in(command, working_directory),
            is_external_source_runtime_command(command),
            is_project_test_command(command),
        ]
        .into_iter()
        .filter(|stage_present| *stage_present)
        .count();
        if delivery_stage_count > 1 {
            return deny(
                "execution_budget",
                "use one tool call per delivery stage so structured evidence can prove ordering: install from source, then run outside the source directory, then run project tests",
            );
        }
    }

    let required_extensions = evidence
        .required_source_scan_extensions
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let source_evidence_paths = evidence
        .source_evidence_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let source_scan_blocked = !required_extensions.is_empty()
        && evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("source compatibility"));
    if source_scan_blocked {
        let source_mutation = matches!(kind, ToolKind::Mutation)
            && !is_source_build_or_install_command(&command.to_ascii_lowercase());
        if source_mutation
            || is_bounded_source_evidence_read(
                command,
                &required_extensions,
                &source_evidence_paths,
            )
            || is_repository_alias_discovery_command(command, &required_extensions)
            || is_repository_source_scan_command(command, &required_extensions)
        {
            return PolicyDecision::Allow;
        }
        return deny(
            "execution_budget",
            "before another build or install, derive every local import alias from the repository and complete the required clean residual scan across all observed source/build-input extensions; derive candidate spellings from exact failures, repository references, or a language adapter, write matches to a temporary results file, preserve the grep/rg status, reject status greater than 1, and finish with `test ! -s`",
        );
    }

    if remaining_model_rounds > 8 && !time_finalization_window && !source_delivery_checkpoint {
        return PolicyDecision::Allow;
    }
    if evidence.completed && (evidence.require_action || evidence.outcome_count > 0) {
        return deny(
            "execution_budget",
            "completion evidence is already satisfied; finalize without another tool call",
        );
    }

    if source_delivery_checkpoint {
        let latest_tool_failed = evidence
            .last_failure_sequence
            .is_some_and(|sequence| sequence == evidence.outcome_count);
        if let Some(runner) = evidence.missing_test_runner.as_deref() {
            let lower = command.to_ascii_lowercase();
            if is_dependency_install_command(&lower) && lower.contains(runner) {
                return PolicyDecision::Allow;
            }
            return deny(
                "execution_budget",
                &format!(
                    "the project test runner `{runner}` is unavailable; install it and rerun the focused project tests before source edits or scope expansion"
                ),
            );
        }
        let source_floor = evidence.last_source_mutation_sequence.unwrap_or(0);
        let lower_command = command.to_ascii_lowercase();
        let repair_mutation = evidence
            .last_failure_sequence
            .is_some_and(|sequence| sequence > source_floor)
            && matches!(kind, ToolKind::Mutation)
            && !is_dependency_install_command(&lower_command);
        let failure_diagnostic = latest_tool_failed && matches!(kind, ToolKind::ReadOnly);
        let dependency_recovery =
            latest_tool_failed && is_dependency_install_command(&lower_command);
        let source_install_missing = !evidence
            .last_source_install_sequence
            .is_some_and(|sequence| sequence > source_floor);
        let external_runtime_missing = match (
            evidence.last_source_install_sequence,
            evidence.last_external_source_runtime_sequence,
        ) {
            (Some(install), Some(runtime)) => runtime <= install,
            _ => true,
        };
        if source_install_missing {
            if is_source_install_command_in(command, working_directory)
                || dependency_recovery
                || repair_mutation
                || failure_diagnostic
            {
                return PolicyDecision::Allow;
            }
            return deny(
                "execution_budget",
                "the source-delivery checkpoint has been reached; install the current source revision before another edit, scan, build-only command, or scope expansion",
            );
        }
        if external_runtime_missing {
            if is_external_source_runtime_command(command)
                || dependency_recovery
                || repair_mutation
                || failure_diagnostic
            {
                return PolicyDecision::Allow;
            }
            return deny(
                "execution_budget",
                "the source-delivery checkpoint has been reached; run the installed package from outside the source directory before another edit, scan, or scope expansion",
            );
        }

        let project_test_due = !evidence
            .last_successful_project_test_sequence
            .is_some_and(|sequence| sequence > source_floor);
        if !source_install_missing
            && !external_runtime_missing
            && project_test_due
            && is_project_test_command(command)
        {
            return PolicyDecision::Allow;
        }
        if !source_install_missing
            && !external_runtime_missing
            && project_test_due
            && !is_project_test_command(command)
            && !latest_tool_failed
        {
            return deny(
                "execution_budget",
                "the source-build midpoint has been reached; run the focused project tests now before expanding or editing the scope",
            );
        }
    }

    let time_read_only_exhausted =
        wall_time.is_some_and(|(remaining, total)| total > 0 && remaining <= (total / 6).max(60));
    if (remaining_model_rounds <= 3 || time_read_only_exhausted)
        && matches!(kind, ToolKind::ReadOnly)
    {
        return deny(
            "execution_budget",
            "read-only exploration is exhausted; perform the required repair or final verification",
        );
    }
    PolicyDecision::Allow
}

#[derive(Debug, Clone)]
pub struct CompletionGate {
    require_action: bool,
    outcome_count: u64,
    last_mutation_sequence: Option<u64>,
    last_successful_verification_sequence: Option<u64>,
    last_failure_sequence: Option<u64>,
    last_service_start_sequence: Option<u64>,
    last_service_pid_evidence_sequence: Option<u64>,
    last_service_log_evidence_sequence: Option<u64>,
    last_bounded_probe_sequence: Option<u64>,
    last_failed_verification_command: Option<String>,
    scope_narrowing_sequence: Option<u64>,
    source_compatibility_audit_required: bool,
    source_delivery_required: bool,
    project_tests_required: bool,
    required_source_scan_extensions: BTreeSet<String>,
    source_evidence_paths: BTreeSet<String>,
    last_source_mutation_sequence: Option<u64>,
    last_source_scan_sequence: Option<u64>,
    last_failed_clean_source_scan_sequence: Option<u64>,
    last_source_install_sequence: Option<u64>,
    last_external_source_runtime_sequence: Option<u64>,
    last_project_test_sequence: Option<u64>,
    last_successful_project_test_sequence: Option<u64>,
    missing_test_runner: Option<String>,
}

impl Default for CompletionGate {
    fn default() -> Self {
        Self::new(false)
    }
}

impl CompletionGate {
    pub fn new(require_action: bool) -> Self {
        Self::new_with_source_requirements(require_action, false, false, false)
    }

    pub fn new_for_instruction(require_action: bool, instruction: &str) -> Self {
        let lower = instruction.to_ascii_lowercase();
        let source_compatibility_audit_required = contains_any(
            &lower,
            &[
                "incompatib",
                "compatibility",
                "deprecated",
                "source migration",
                "migrate the source",
                "runtime upgrade",
                "dependency upgrade",
                "api upgrade",
                "不兼容",
                "兼容",
                "已移除",
                "弃用",
                "废弃",
                "源码迁移",
                "运行时升级",
                "依赖升级",
                "api 升级",
            ],
        );
        let source_delivery_required = (lower.contains("install") || lower.contains("安装"))
            && contains_any(
                &lower,
                &[
                    "from source",
                    "source build",
                    "compile extension",
                    "compiled extension",
                    "build extension",
                    "从源码",
                    "源码构建",
                    "编译扩展",
                    "构建扩展",
                ],
            );
        let project_tests_required = source_delivery_required
            && contains_any(
                &lower,
                &[
                    "test",
                    "pytest",
                    "test suite",
                    "项目测试",
                    "测试套件",
                    "完整测试",
                ],
            );
        Self::new_with_source_requirements(
            require_action,
            source_compatibility_audit_required,
            source_delivery_required,
            project_tests_required,
        )
    }

    fn new_with_source_requirements(
        require_action: bool,
        source_compatibility_audit_required: bool,
        source_delivery_required: bool,
        project_tests_required: bool,
    ) -> Self {
        Self {
            require_action,
            outcome_count: 0,
            last_mutation_sequence: None,
            last_successful_verification_sequence: None,
            last_failure_sequence: None,
            last_service_start_sequence: None,
            last_service_pid_evidence_sequence: None,
            last_service_log_evidence_sequence: None,
            last_bounded_probe_sequence: None,
            last_failed_verification_command: None,
            scope_narrowing_sequence: None,
            source_compatibility_audit_required,
            source_delivery_required,
            project_tests_required,
            required_source_scan_extensions: BTreeSet::new(),
            source_evidence_paths: BTreeSet::new(),
            last_source_mutation_sequence: None,
            last_source_scan_sequence: None,
            last_failed_clean_source_scan_sequence: None,
            last_source_install_sequence: None,
            last_external_source_runtime_sequence: None,
            last_project_test_sequence: None,
            last_successful_project_test_sequence: None,
            missing_test_runner: None,
        }
    }

    pub fn record(&mut self, outcome: &ToolOutcome) {
        self.outcome_count += 1;
        if is_source_mutation(outcome) {
            self.last_source_mutation_sequence = Some(outcome.sequence);
        }
        if self.source_compatibility_audit_required {
            self.required_source_scan_extensions
                .extend(observed_build_input_extensions(outcome));
            self.source_evidence_paths
                .extend(observed_source_evidence_paths(outcome));
            if is_clean_repository_source_scan(outcome, &self.required_source_scan_extensions) {
                self.last_source_scan_sequence = Some(outcome.sequence);
                self.last_failed_clean_source_scan_sequence = None;
            } else if !outcome.succeeded()
                && is_repository_source_scan_attempt(
                    &outcome.command,
                    &self.required_source_scan_extensions,
                )
                && source_scan_output_claims_clean(outcome)
            {
                self.last_failed_clean_source_scan_sequence = Some(outcome.sequence);
            }
        }
        if self.source_delivery_required && outcome.succeeded() {
            if is_source_install_command_in(&outcome.command, outcome.working_directory.as_deref())
            {
                self.last_source_install_sequence = Some(outcome.sequence);
            } else if self.last_source_install_sequence.is_some()
                && is_external_source_runtime_command(&outcome.command)
            {
                self.last_external_source_runtime_sequence = Some(outcome.sequence);
                if self.last_failed_verification_command.is_none() {
                    self.last_successful_verification_sequence = Some(outcome.sequence);
                }
            }
        }
        if outcome.succeeded()
            && self.missing_test_runner.as_deref().is_some_and(|runner| {
                is_dependency_install_command(&outcome.command.to_ascii_lowercase())
                    && outcome.command.to_ascii_lowercase().contains(runner)
            })
        {
            self.missing_test_runner = None;
        }
        if matches!(outcome.kind, ToolKind::Mutation) {
            self.last_mutation_sequence = Some(outcome.sequence);
        }
        if matches!(outcome.kind, ToolKind::BackgroundServiceStart) && outcome.succeeded() {
            self.last_service_start_sequence = Some(outcome.sequence);
        }
        if self.last_service_start_sequence.is_some() && outcome.succeeded() {
            if has_service_pid_evidence(&outcome.command, &outcome.stdout) {
                self.last_service_pid_evidence_sequence = Some(outcome.sequence);
            }
            if has_service_log_evidence(&outcome.command) {
                self.last_service_log_evidence_sequence = Some(outcome.sequence);
            }
        }
        if matches!(outcome.kind, ToolKind::Verification) {
            if is_project_test_command(&outcome.command) {
                self.last_project_test_sequence = Some(outcome.sequence);
                if let Some(runner) = detect_missing_test_runner(outcome) {
                    self.missing_test_runner = Some(runner.to_owned());
                } else if outcome.succeeded() {
                    self.missing_test_runner = None;
                    self.last_successful_project_test_sequence = Some(outcome.sequence);
                }
            }
            if outcome.succeeded() {
                let scope_was_narrowed = self
                    .last_failed_verification_command
                    .as_deref()
                    .is_some_and(|failed_command| {
                        verification_scope_restriction_count(&outcome.command)
                            > verification_scope_restriction_count(failed_command)
                    });
                if scope_was_narrowed {
                    self.scope_narrowing_sequence = Some(outcome.sequence);
                    self.last_failure_sequence = Some(outcome.sequence);
                } else {
                    self.last_successful_verification_sequence = Some(outcome.sequence);
                    self.last_failed_verification_command = None;
                    self.scope_narrowing_sequence = None;
                }
            } else {
                self.last_failed_verification_command = Some(outcome.command.clone());
            }
        }
        if matches!(outcome.kind, ToolKind::RuntimeProbe)
            && outcome.succeeded()
            && self.last_failed_verification_command.is_none()
            && self
                .last_mutation_sequence
                .is_some_and(|sequence| outcome.sequence > sequence)
        {
            self.last_successful_verification_sequence = Some(outcome.sequence);
        }
        if matches!(outcome.kind, ToolKind::FunctionalProbe { bounded: true })
            && outcome.succeeded()
        {
            self.last_bounded_probe_sequence = Some(outcome.sequence);
            if self.last_failed_verification_command.is_none() {
                self.last_successful_verification_sequence = Some(outcome.sequence);
            }
        }
        // A failed read (grep with no matches, or output that merely contains
        // an "error:"-looking string) changes no workspace state and must not
        // force delivery-grade verification before a final answer — that is
        // what turned pure-analysis turns into reject/re-answer loops.
        if !outcome.succeeded() && !matches!(outcome.kind, ToolKind::ReadOnly) {
            self.last_failure_sequence = Some(outcome.sequence);
        }
    }

    pub fn evidence(&self) -> CompletionEvidence {
        let mut blockers = Vec::new();
        if self.scope_narrowing_sequence.is_some() {
            blockers.push(
                "verification scope was narrowed after a failure; rerun the original scope after repairing it"
                    .to_owned(),
            );
        }
        let verification_floor = self
            .last_mutation_sequence
            .into_iter()
            .chain(self.last_failure_sequence)
            .max()
            .unwrap_or(0);

        let verification_required = self.require_action
            || self.last_mutation_sequence.is_some()
            || self.last_failure_sequence.is_some();
        if verification_required {
            match self.last_successful_verification_sequence {
                Some(sequence) if sequence > verification_floor => {}
                Some(_) => blockers.push(
                    "successful verification must be later than the last mutation or failed tool"
                        .to_owned(),
                ),
                None => {
                    blockers.push("at least one successful verification is required".to_owned())
                }
            }
        }

        if let Some(service_sequence) = self.last_service_start_sequence {
            if !self
                .last_service_pid_evidence_sequence
                .is_some_and(|sequence| sequence >= service_sequence)
            {
                blockers.push(
                    "background services require a recorded PID, pidfile, or process handle"
                        .to_owned(),
                );
            }
            if !self
                .last_service_log_evidence_sequence
                .is_some_and(|sequence| sequence >= service_sequence)
            {
                blockers.push("background services require an explicit log destination".to_owned());
            }
            match self.last_bounded_probe_sequence {
                Some(probe_sequence)
                    if probe_sequence > service_sequence && probe_sequence > verification_floor => {
                }
                _ => blockers.push(
                    "background services require a later successful bounded functional probe"
                        .to_owned(),
                ),
            }
        }

        if self.source_compatibility_audit_required
            && !self.required_source_scan_extensions.is_empty()
        {
            if let Some(source_mutation_sequence) = self.last_source_mutation_sequence {
                if self
                    .last_failed_clean_source_scan_sequence
                    .is_some_and(|sequence| sequence >= source_mutation_sequence)
                {
                    blockers.push(
                        "source compatibility residual scan reported zero residual matches but exited nonzero; rerun once by writing matches to a temporary results file, preserving the grep/rg status, rejecting status greater than 1, and finishing with `test ! -s`"
                            .to_owned(),
                    );
                }
                if !self
                    .last_source_scan_sequence
                    .is_some_and(|sequence| sequence > source_mutation_sequence)
                {
                    blockers.push(format!(
                        "source compatibility work requires a clean repository-wide residual scan after the last source edit covering these build-input extensions: {}; make the scan command return 0 only when no unresolved matches remain",
                        self.required_source_scan_extensions
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }

        if self.source_delivery_required {
            let source_floor = self.last_source_mutation_sequence.unwrap_or(0);
            match self.last_source_install_sequence {
                Some(sequence) if sequence > source_floor => {}
                _ => blockers.push(
                    "source-build delivery requires a successful install from source after the last source edit"
                        .to_owned(),
                ),
            }
            match (self.last_source_install_sequence, self.last_external_source_runtime_sequence) {
                (Some(install), Some(runtime)) if runtime > install => {}
                _ => blockers.push(
                    "source-build delivery requires a successful runtime check outside the source directory after installation"
                        .to_owned(),
                ),
            }
            if self.project_tests_required {
                let project_test_floor = self
                    .last_source_mutation_sequence
                    .into_iter()
                    .chain(self.last_source_install_sequence)
                    .chain(self.last_external_source_runtime_sequence)
                    .max()
                    .unwrap_or(0);
                if !self
                    .last_successful_project_test_sequence
                    .is_some_and(|sequence| sequence > project_test_floor)
                {
                    blockers.push(
                        "source-build delivery requires successful project tests after installation and the external runtime check"
                            .to_owned(),
                    );
                }
            }
        }

        CompletionEvidence {
            require_action: self.require_action,
            outcome_count: self.outcome_count,
            last_mutation_sequence: self.last_mutation_sequence,
            last_successful_verification_sequence: self.last_successful_verification_sequence,
            last_failure_sequence: self.last_failure_sequence,
            last_service_start_sequence: self.last_service_start_sequence,
            last_service_pid_evidence_sequence: self.last_service_pid_evidence_sequence,
            last_service_log_evidence_sequence: self.last_service_log_evidence_sequence,
            last_bounded_probe_sequence: self.last_bounded_probe_sequence,
            required_source_scan_extensions: self
                .required_source_scan_extensions
                .iter()
                .cloned()
                .collect(),
            source_evidence_paths: self.source_evidence_paths.iter().cloned().collect(),
            last_source_scan_sequence: self.last_source_scan_sequence,
            source_delivery_required: self.source_delivery_required,
            project_tests_required: self.project_tests_required,
            last_source_mutation_sequence: self.last_source_mutation_sequence,
            last_source_install_sequence: self.last_source_install_sequence,
            last_external_source_runtime_sequence: self.last_external_source_runtime_sequence,
            last_project_test_sequence: self.last_project_test_sequence,
            last_successful_project_test_sequence: self.last_successful_project_test_sequence,
            missing_test_runner: self.missing_test_runner.clone(),
            completed: blockers.is_empty(),
            blockers,
        }
    }
}

const SOURCE_INPUT_EXTENSIONS: &[&str] = &[
    ".py", ".pyx", ".pxd", ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".rs", ".go", ".java", ".kt",
    ".kts", ".swift", ".m", ".mm", ".cs", ".fs", ".scala", ".proto",
];

fn observed_build_input_extensions(outcome: &ToolOutcome) -> BTreeSet<String> {
    let lower_command = outcome.command.to_ascii_lowercase();
    let inspects_build_config = contains_any(
        &lower_command,
        &[
            "setup.py",
            "pyproject.toml",
            "cargo.toml",
            "cmakelists.txt",
            "meson.build",
            "makefile",
            "package.json",
            "build.gradle",
            "pom.xml",
        ],
    );
    let build_reports_inputs = outcome.stdout.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        contains_any(&line, &["compiling ", "cythonizing ", "building extension"])
    });
    let mut extensions = BTreeSet::new();
    if inspects_build_config || build_reports_inputs {
        collect_source_extensions(&outcome.stdout, &mut extensions);
        collect_source_extensions(&outcome.stderr, &mut extensions);
    }
    if is_source_mutation(outcome) {
        collect_source_extensions(&outcome.command, &mut extensions);
    }
    extensions
}

fn observed_source_evidence_paths(outcome: &ToolOutcome) -> BTreeSet<String> {
    let lower_command = outcome.command.to_ascii_lowercase();
    let inspects_build_config = contains_any(
        &lower_command,
        &[
            "setup.py",
            "pyproject.toml",
            "cargo.toml",
            "cmakelists.txt",
            "meson.build",
            "makefile",
            "package.json",
            "build.gradle",
            "pom.xml",
        ],
    );
    let mut paths = BTreeSet::new();
    if is_source_mutation(outcome) {
        collect_source_paths(&outcome.command, &mut paths);
    }
    if inspects_build_config {
        collect_source_paths(&outcome.stdout, &mut paths);
        collect_source_paths(&outcome.stderr, &mut paths);
    }
    paths
}

fn collect_source_paths(text: &str, paths: &mut BTreeSet<String>) {
    for token in text.split_whitespace() {
        let candidate = token
            .trim_matches(|character: char| {
                matches!(
                    character,
                    '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
                )
            })
            .replace('\\', "/");
        let lower = candidate.to_ascii_lowercase();
        if SOURCE_INPUT_EXTENSIONS
            .iter()
            .any(|extension| lower.len() > extension.len() && lower.ends_with(extension))
            && !has_ignored_source_component(&lower)
        {
            paths.insert(candidate);
        }
    }
}

fn collect_source_extensions(text: &str, extensions: &mut BTreeSet<String>) {
    let lower = text.to_ascii_lowercase();
    for extension in SOURCE_INPUT_EXTENSIONS {
        if lower.match_indices(extension).any(|(index, _)| {
            lower[index + extension.len()..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_ascii_alphanumeric())
        }) {
            extensions.insert((*extension).to_owned());
        }
    }
}

fn is_source_mutation(outcome: &ToolOutcome) -> bool {
    if !matches!(outcome.kind, ToolKind::Mutation)
        || outcome
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("policy denied command"))
    {
        return false;
    }

    let lower_command = outcome.command.to_ascii_lowercase();
    has_explicit_file_mutation(&lower_command)
        || (outcome.succeeded() && !is_dependency_install_command(&lower_command))
}

fn has_explicit_file_mutation(command: &str) -> bool {
    contains_any(
        command,
        &[
            "apply_patch",
            "sed -i",
            "perl -pi",
            "tee ",
            "cat >",
            "cat >>",
            "touch ",
            "rm ",
            "mv ",
            "cp ",
            "git apply",
        ],
    )
}

fn detect_missing_test_runner(outcome: &ToolOutcome) -> Option<&'static str> {
    if !is_project_test_command(&outcome.command) {
        return None;
    }
    let command = outcome.command.to_ascii_lowercase();
    let output = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    for runner in [
        "pytest", "jest", "vitest", "cargo", "npm", "pnpm", "yarn", "bun", "go", "mvn", "gradle",
        "dotnet", "swift",
    ] {
        let command_uses_runner = command.contains(runner);
        let missing_module = output.contains(&format!("no module named {runner}"));
        let missing_command = output.contains(&format!("{runner}: command not found"))
            || output.contains(&format!("{runner}: not found"));
        if command_uses_runner && (missing_module || missing_command) {
            return Some(runner);
        }
    }
    None
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn simple_and_segments(command: &str) -> Option<Vec<Vec<String>>> {
    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut token_started = false;
    let mut quote = None;
    let mut escaped = false;
    let mut chars = command.chars().peekable();

    let finish_token = |tokens: &mut Vec<String>, token: &mut String, started: &mut bool| {
        if *started {
            tokens.push(std::mem::take(token));
            *started = false;
        }
    };
    let finish_segment = |segments: &mut Vec<Vec<String>>, tokens: &mut Vec<String>| {
        if tokens.is_empty() {
            return false;
        }
        segments.push(std::mem::take(tokens));
        true
    };

    while let Some(character) = chars.next() {
        if escaped {
            token.push(character);
            token_started = true;
            escaped = false;
            continue;
        }
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    token.push(character);
                }
                token_started = true;
            }
            Some('"') => {
                if character == '"' {
                    quote = None;
                } else if character == '\\' {
                    if chars
                        .peek()
                        .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`'))
                    {
                        escaped = true;
                    } else {
                        token.push(character);
                    }
                } else if character == '`'
                    || character == '$' && chars.peek().is_some_and(|next| *next == '(')
                {
                    return None;
                } else {
                    token.push(character);
                }
                token_started = true;
            }
            Some(_) => unreachable!(),
            None => match character {
                '\'' | '"' => {
                    quote = Some(character);
                    token_started = true;
                }
                '\\' => {
                    if chars.peek().is_some_and(|next| {
                        matches!(next, '\\' | '\'' | '"' | ' ' | '\t' | '&' | '|' | ';')
                    }) {
                        escaped = true;
                    } else {
                        token.push(character);
                        token_started = true;
                    }
                }
                ' ' | '\t' | '\r' => {
                    finish_token(&mut tokens, &mut token, &mut token_started);
                }
                '&' if chars.peek().is_some_and(|next| *next == '&') => {
                    chars.next();
                    finish_token(&mut tokens, &mut token, &mut token_started);
                    if !finish_segment(&mut segments, &mut tokens) {
                        return None;
                    }
                }
                '&' if tokens.is_empty()
                    && !token_started
                    && chars.peek().is_some_and(|next| next.is_whitespace()) =>
                {
                    tokens.push("&".to_owned());
                }
                ';' | '\n' | '|' | '&' | '(' | ')' | '`' => return None,
                '$' if chars.peek().is_some_and(|next| *next == '(') => return None,
                _ => {
                    token.push(character);
                    token_started = true;
                }
            },
        }
    }
    if escaped || quote.is_some() {
        return None;
    }
    finish_token(&mut tokens, &mut token, &mut token_started);
    if !finish_segment(&mut segments, &mut tokens) {
        return None;
    }
    Some(segments)
}

#[cfg(test)]
fn is_source_install_command(command: &str) -> bool {
    is_source_install_command_in(command, None)
}

fn is_source_install_command_in(command: &str, working_directory: Option<&str>) -> bool {
    fn is_current_directory(target: &str) -> bool {
        matches!(target, "." | "./")
    }

    fn pip_installs_current_source(args: &[String]) -> bool {
        const OPTIONS_WITH_VALUES: &[&str] = &[
            "-c",
            "--constraint",
            "-f",
            "--find-links",
            "-i",
            "--index-url",
            "--extra-index-url",
            "--trusted-host",
            "-r",
            "--requirement",
            "--src",
            "-t",
            "--target",
            "--root",
            "--prefix",
            "--python",
            "--config-settings",
            "--cache-dir",
            "--platform",
            "--python-version",
            "--implementation",
            "--abi",
            "--upgrade-strategy",
            "--root-user-action",
            "--progress-bar",
            "--report",
            "--timeout",
            "--retries",
            "--cert",
            "--client-cert",
            "--no-binary",
            "--only-binary",
            "--link-mode",
            "--index-strategy",
            "--proxy",
            "--exists-action",
            "--keyring-provider",
            "--use-feature",
            "--use-deprecated",
        ];
        const OPTIONS_WITHOUT_VALUES: &[&str] = &[
            "--no-deps",
            "--no-index",
            "--no-build-isolation",
            "--use-pep517",
            "--no-use-pep517",
            "--pre",
            "--upgrade",
            "-u",
            "--force-reinstall",
            "--ignore-installed",
            "--user",
            "--compile",
            "--no-compile",
            "--no-clean",
            "--break-system-packages",
            "--require-hashes",
            "--prefer-binary",
            "-q",
            "--quiet",
            "-v",
            "--verbose",
            "--disable-pip-version-check",
            "--isolated",
            "--no-input",
            "--system",
            "--refresh",
            "--reinstall",
            "--no-cache-dir",
            "--no-color",
        ];
        const NON_INSTALLING_OPTIONS: &[&str] = &["-h", "--help", "--dry-run"];

        let mut index = 0;
        let mut found_current_source = false;
        let mut positional_only = false;
        while index < args.len() {
            let argument = args[index].as_str();
            if NON_INSTALLING_OPTIONS.contains(&argument) {
                return false;
            }
            if argument == "--" {
                positional_only = true;
                index += 1;
                continue;
            }
            if matches!(argument, "-e" | "--editable") {
                let Some(target) = args.get(index + 1) else {
                    return false;
                };
                found_current_source |= is_current_directory(target);
                index += 2;
                continue;
            }
            if let Some(target) = argument
                .strip_prefix("--editable=")
                .or_else(|| argument.strip_prefix("-e="))
            {
                found_current_source |= is_current_directory(target);
                index += 1;
                continue;
            }
            if let Some(target) = argument.strip_prefix("-e") {
                if target.is_empty() {
                    return false;
                }
                found_current_source |= is_current_directory(target);
                index += 1;
                continue;
            }
            if !positional_only && OPTIONS_WITH_VALUES.contains(&argument) {
                if args.get(index + 1).is_none() {
                    return false;
                }
                index += 2;
                continue;
            }
            if !positional_only
                && OPTIONS_WITH_VALUES
                    .iter()
                    .any(|option| argument.starts_with(&format!("{option}=")))
            {
                index += 1;
                continue;
            }
            if !positional_only && OPTIONS_WITHOUT_VALUES.contains(&argument) {
                index += 1;
                continue;
            }
            if !positional_only
                && argument.starts_with('-')
                && argument[1..]
                    .chars()
                    .all(|character| matches!(character, 'q' | 'v'))
            {
                index += 1;
                continue;
            }
            if !positional_only && argument.starts_with('-') {
                return false;
            }
            if is_current_directory(argument) {
                found_current_source = true;
            }
            index += 1;
        }
        found_current_source
    }

    fn is_environment_assignment(token: &str) -> bool {
        token.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
    }

    fn is_py_version_selector(token: &str) -> bool {
        let Some(version) = token.strip_prefix('-') else {
            return false;
        };
        !version.is_empty()
            && !version.starts_with('.')
            && !version.ends_with('.')
            && version
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.')
            && !version.contains("..")
    }

    let Some(segments) = simple_and_segments(command) else {
        return false;
    };
    let source_root = working_directory
        .map(PathBuf::from)
        .map(|path| normalize_lexical_path(&path));
    let mut effective_directory = source_root.clone();
    let mut inside_source_workspace = true;

    for tokens in segments {
        let lower_tokens = tokens
            .iter()
            .map(|token| token.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let mut start = tokens
            .iter()
            .position(|token| !is_environment_assignment(token))
            .unwrap_or(tokens.len());
        if lower_tokens.get(start).is_some_and(|token| token == "&") {
            start += 1;
        }
        let Some(executable) = lower_tokens
            .get(start)
            .and_then(|token| token.rsplit(['/', '\\']).next())
        else {
            continue;
        };

        if executable == "cd" {
            let Some(target) = tokens.get(start + 1) else {
                inside_source_workspace = false;
                effective_directory = None;
                continue;
            };
            if is_current_directory(target) {
                continue;
            }
            let Some(current) = effective_directory.as_ref() else {
                inside_source_workspace = false;
                continue;
            };
            let target_path = Path::new(target);
            let next_directory = if target_path.is_absolute() {
                normalize_lexical_path(target_path)
            } else {
                normalize_lexical_path(&current.join(target_path))
            };
            inside_source_workspace = source_root
                .as_ref()
                .is_some_and(|root| next_directory == *root);
            effective_directory = Some(next_directory);
            continue;
        }
        if executable == "export"
            && lower_tokens[start + 1..]
                .iter()
                .all(|token| is_environment_assignment(token))
        {
            continue;
        }

        let python_module_index = if executable.starts_with("python") {
            Some(start + 1)
        } else if matches!(executable, "py" | "py.exe") {
            Some(
                start
                    + if lower_tokens
                        .get(start + 1)
                        .is_some_and(|token| is_py_version_selector(token))
                    {
                        2
                    } else {
                        1
                    },
            )
        } else {
            None
        };
        let install_index = if matches!(executable, "pip" | "pip3" | "pip.exe" | "pip3.exe")
            && lower_tokens
                .get(start + 1)
                .is_some_and(|token| token == "install")
        {
            Some(start + 1)
        } else if python_module_index.is_some_and(|module_index| {
            lower_tokens
                .get(module_index)
                .is_some_and(|token| token == "-m")
                && lower_tokens
                    .get(module_index + 1)
                    .is_some_and(|token| matches!(token.as_str(), "pip" | "pip3"))
                && lower_tokens
                    .get(module_index + 2)
                    .is_some_and(|token| token == "install")
        }) {
            Some(python_module_index.expect("checked above") + 2)
        } else if executable == "uv"
            && lower_tokens
                .get(start + 1)
                .is_some_and(|token| token == "pip")
            && lower_tokens
                .get(start + 2)
                .is_some_and(|token| token == "install")
        {
            Some(start + 2)
        } else {
            None
        };
        if let Some(install_index) = install_index {
            if inside_source_workspace
                && pip_installs_current_source(&lower_tokens[install_index + 1..])
            {
                return true;
            }
            return false;
        }

        let command_args = &lower_tokens[start + 1..];
        let path_targets_current_setup = |path: &str| {
            if matches!(path, "setup.py" | "./setup.py") {
                return inside_source_workspace;
            }
            let (Some(current), Some(root)) = (effective_directory.as_ref(), source_root.as_ref())
            else {
                return false;
            };
            let path = Path::new(path);
            let resolved = if path.is_absolute() {
                normalize_lexical_path(path)
            } else {
                normalize_lexical_path(&current.join(path))
            };
            resolved == root.join("setup.py")
        };
        let direct_setup = executable == "setup.py" && path_targets_current_setup(&tokens[start]);
        let python_setup = executable.starts_with("python")
            && tokens
                .get(start + 1)
                .is_some_and(|path| path_targets_current_setup(path));
        let non_installing = command_args
            .iter()
            .any(|token| matches!(token.as_str(), "-h" | "--help" | "--dry-run"));
        if !non_installing
            && (direct_setup || python_setup)
            && command_args.iter().any(|token| token == "install")
        {
            return inside_source_workspace;
        }
        if executable == "cargo"
            && command_args.first().is_some_and(|token| token == "install")
            && !non_installing
            && !command_args.iter().any(|token| token == "--list")
            && command_args
                .windows(2)
                .any(|pair| pair[0] == "--path" && is_current_directory(pair[1].as_str()))
        {
            return inside_source_workspace;
        }
        if executable == "npm"
            && command_args.first().is_some_and(|token| token == "install")
            && !non_installing
            && command_args.windows(2).any(|pair| {
                matches!(pair[0].as_str(), "-g" | "--global")
                    && is_current_directory(pair[1].as_str())
            })
        {
            return inside_source_workspace;
        }
        // A successful overall shell command does not prove that an arbitrary
        // earlier segment preserved the effective directory or install state.
        return false;
    }
    false
}

fn is_source_build_or_install_command(command: &str) -> bool {
    is_dependency_install_command(command)
        || contains_any(
            command,
            &[
                "setup.py build",
                "build_ext",
                "cargo build",
                "npm run build",
                "pnpm build",
                "yarn build",
                "bun run build",
                "cmake --build",
                "mvn package",
                "gradle build",
                "gradlew build",
                "go build",
                "docker build",
                "podman build",
                "dotnet build",
                "swift build",
                "xcodebuild",
            ],
        )
}

fn is_external_source_runtime_command(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let external_working_directory = contains_any(
        &lower,
        &[
            "cd /tmp",
            "cd /var/tmp",
            "cd /root",
            "cd / &&",
            "pushd /tmp",
            "mktemp -d",
            "tempfile.mkdtemp",
        ],
    );
    let runtime = contains_any(
        &lower,
        &[
            "python -c ",
            "python3 -c ",
            "node -e ",
            "ruby -e ",
            "perl -e ",
            "java -cp ",
            "dotnet ",
        ],
    );
    external_working_directory && runtime
}

fn is_clean_repository_source_scan(
    outcome: &ToolOutcome,
    required_extensions: &BTreeSet<String>,
) -> bool {
    if required_extensions.is_empty() || !outcome.succeeded() {
        return false;
    }
    is_repository_source_scan_command(&outcome.command, required_extensions)
}

fn is_repository_source_scan_command(
    command: &str,
    required_extensions: &BTreeSet<String>,
) -> bool {
    if !is_repository_source_scan_attempt(command, required_extensions) {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    source_scan_has_clean_exit_contract(&lower)
}

fn is_repository_source_scan_attempt(
    command: &str,
    required_extensions: &BTreeSet<String>,
) -> bool {
    if required_extensions.is_empty() {
        return false;
    }
    let lower = command.to_ascii_lowercase();
    let repository_wide = lower.contains("rg ")
        || lower.contains("grep -r")
        || lower.contains("grep --recursive")
        || lower.contains("find ")
        || lower.contains("os.walk(");
    if !repository_wide {
        return false;
    }
    let has_extension_filter = contains_any(
        &lower,
        &["--include", "--glob", " -g ", " -name ", " -path "],
    );
    if !has_extension_filter {
        return true;
    }
    required_extensions
        .iter()
        .all(|extension| lower.contains(extension))
}

fn source_scan_output_claims_clean(outcome: &ToolOutcome) -> bool {
    let output = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    let claims_clean = contains_any(
        &output,
        &[
            "0 unresolved",
            "zero residual",
            "no unresolved",
            "scan clean",
            "all clean",
        ],
    );
    claims_clean
        && !contains_any(
            &output,
            &["issues remain", "unresolved matches remain", "not clean"],
        )
}

fn is_repository_alias_discovery_command(
    command: &str,
    required_extensions: &BTreeSet<String>,
) -> bool {
    let lower = command.to_ascii_lowercase();
    let repository_wide = lower.contains("rg ")
        || lower.starts_with("grep ")
        || lower.contains("grep -r")
        || lower.contains("grep --recursive")
        || lower.contains("find ")
        || lower.contains("os.walk(");
    let alias_focused = contains_any(
        &lower,
        &[
            "^(import|from)",
            "import ",
            "from ",
            " require(",
            "require(",
        ],
    );
    let extension_scoped = contains_any(&lower, &["--include", "--glob", " -g ", " glob="])
        && required_extensions
            .iter()
            .all(|extension| lower.contains(extension));
    repository_wide && alias_focused && extension_scoped
}

fn is_bounded_source_evidence_read(
    command: &str,
    required_extensions: &BTreeSet<String>,
    source_evidence_paths: &BTreeSet<String>,
) -> bool {
    let Some(path) = command.trim().strip_prefix("read_file ") else {
        return false;
    };
    let path = path.trim_matches(['\'', '"']).replace('\\', "/");
    let lower = path.to_ascii_lowercase();
    if path.split_whitespace().count() != 1
        || has_ignored_source_component(&lower)
        || !required_extensions
            .iter()
            .any(|extension| lower.ends_with(extension))
    {
        return false;
    }
    source_evidence_paths.iter().any(|observed| {
        let observed = observed.replace('\\', "/").to_ascii_lowercase();
        lower == observed
            || lower.ends_with(&format!("/{observed}"))
            || observed.ends_with(&format!("/{lower}"))
            || source_parent(&lower)
                .zip(source_parent(&observed))
                .is_some_and(|(candidate_parent, observed_parent)| {
                    candidate_parent == observed_parent
                        || candidate_parent.ends_with(&format!("/{observed_parent}"))
                        || observed_parent.ends_with(&format!("/{candidate_parent}"))
                })
    })
}

fn source_parent(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

fn has_ignored_source_component(path: &str) -> bool {
    path.split(['/', '\\']).any(|component| {
        matches!(
            component,
            ".git" | ".venv" | "venv" | "node_modules" | "target" | "__pycache__"
        )
    })
}

fn source_scan_has_clean_exit_contract(command: &str) -> bool {
    let structured_exit =
        command.contains("os.walk(") && contains_any(command, &["sys.exit(", "raise systemexit"]);
    let robust_shell_exit = command.contains("test ! -s")
        && contains_any(command, &["=$?", "=\"$?\""])
        && contains_any(command, &["-gt 1", "-le 1", "> 1", ">1", "case "])
        && contains_any(
            command,
            &[
                "exit $", "exit \"$", "exit 1", "exit 2", "return 1", "return 2",
            ],
        );
    structured_exit || robust_shell_exit
}

fn verification_scope_restriction_count(command: &str) -> usize {
    command
        .split_whitespace()
        .filter(|token| {
            let token = token.to_ascii_lowercase();
            token == "-k"
                || token == "--skip"
                || token == "--exclude"
                || token == "--filter"
                || token == "--grep"
                || token.starts_with("--ignore=")
                || token.starts_with("--ignore-glob=")
                || token.starts_with("--exclude=")
                || token.starts_with("--filter=")
                || token.starts_with("--grep=")
        })
        .count()
}

fn has_service_pid_evidence(command: &str, stdout: &str) -> bool {
    let command = command.to_ascii_lowercase();
    let stdout = stdout.to_ascii_lowercase();
    contains_any(
        &command,
        &["$!", ".pid", "pidfile", "docker run -d", "podman run -d"],
    ) || stdout.lines().any(|line| {
        let line = line.trim();
        line.parse::<u64>().is_ok() || line.starts_with("pid=") || line.starts_with("pid ")
    })
}

fn has_service_log_evidence(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    contains_any(
        &command,
        &[
            ".log",
            "--log-file",
            "--log-path",
            "docker logs",
            "podman logs",
            "journalctl",
        ],
    )
}

#[derive(Debug, Clone)]
pub struct ProgressTracker {
    read_only_limit: u32,
    consecutive_read_only: u32,
    mutation_seen: bool,
    read_only_exhausted: bool,
}

impl ProgressTracker {
    pub fn new(read_only_limit: u32) -> Self {
        Self {
            read_only_limit: read_only_limit.max(1),
            consecutive_read_only: 0,
            mutation_seen: false,
            read_only_exhausted: false,
        }
    }

    pub fn record(&mut self, outcome: &ToolOutcome) -> Option<String> {
        match outcome.kind {
            ToolKind::Mutation => {
                self.mutation_seen = true;
                self.consecutive_read_only = 0;
                self.read_only_exhausted = false;
            }
            ToolKind::ReadOnly | ToolKind::RuntimeProbe
                if outcome.succeeded() && !self.mutation_seen =>
            {
                self.consecutive_read_only += 1;
                if self.consecutive_read_only >= self.read_only_limit && !self.read_only_exhausted {
                    self.read_only_exhausted = true;
                    return Some(
                        "The bounded inspection budget is exhausted and no implementation has \
started. Use the evidence already collected to create the smallest candidate implementation \
now, then run a real verification. Continue inspecting only if you can name a specific missing \
fact that blocks implementation."
                            .to_owned(),
                    );
                }
            }
            _ => {
                self.consecutive_read_only = 0;
                self.read_only_exhausted = false;
            }
        }
        None
    }

    pub fn read_only_exhausted_before_mutation(&self) -> bool {
        !self.mutation_seen && self.read_only_exhausted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(sequence: u64, kind: ToolKind, return_code: i32) -> ToolOutcome {
        ToolOutcome {
            request_id: format!("tool-{sequence}"),
            command: "command".to_owned(),
            working_directory: None,
            kind,
            sequence,
            started_at_ms: sequence * 10,
            finished_at_ms: sequence * 10 + 1,
            return_code: Some(return_code),
            stdout: String::new(),
            stderr: String::new(),
            error: None,
            semantic_failure: false,
        }
    }

    #[test]
    fn contract_hash_is_stable_sha256() {
        assert!(!EXECUTION_CONTRACT.trim().is_empty());
        assert_eq!(execution_contract_sha256().len(), 64);
        assert_eq!(execution_contract_sha256(), execution_contract_sha256());
    }

    #[test]
    fn system_prompt_reports_the_effective_network_capability() {
        let public = build_system_prompt(true);
        assert!(public.contains("External network access is available"));
        assert!(!public.contains("Network access is denied by default"));

        let isolated = build_system_prompt(false);
        assert!(isolated.contains("Network access is denied by default"));
    }

    #[test]
    fn product_prompt_names_the_workspace_write_boundary() {
        let prompt = build_product_system_prompt(false);
        assert!(prompt.contains("writable workspace"));
        assert!(prompt.contains("$TMPDIR"));
        assert!(!prompt.contains("hidden verifiers"));
    }

    #[test]
    fn benchmark_policy_denies_hidden_verifier_solution_and_secrets() {
        let policy = BenchmarkPolicy::default();
        for command in [
            "bash /tests/test.sh",
            "find / -name 'test*' -o -name 'verify*' -o -name 'solution*'",
            "cat /solution/answer.txt",
            "printenv",
            "cat ~/.ssh/id_ed25519",
        ] {
            assert!(!policy.evaluate_command(command).is_allowed(), "{command}");
        }
    }

    #[test]
    fn benchmark_policy_allows_only_loopback_network_by_default() {
        let policy = BenchmarkPolicy::default();
        assert!(policy
            .evaluate_command("curl --max-time 2 http://127.0.0.1:8080/health")
            .is_allowed());
        assert!(policy
            .evaluate_command("curl --max-time 2 http://localhost:8080/health")
            .is_allowed());
        assert!(!policy
            .evaluate_command("curl https://example.com/archive.tar.gz")
            .is_allowed());
        assert!(!policy.evaluate_command("pip install requests").is_allowed());
        assert!(policy
            .evaluate_command("pip install --no-index ./vendor/package.whl")
            .is_allowed());
    }

    #[test]
    fn product_policy_allows_normal_project_test_and_solution_paths() {
        let policy = ProductPolicy::default();
        assert!(policy
            .evaluate_command("pytest /Users/leo/project/tests/test_api.py")
            .is_allowed());
        assert!(policy
            .evaluate_command("cat /Users/leo/project/solution/solver.py")
            .is_allowed());
        assert!(!policy.evaluate_command("printenv").is_allowed());
    }

    #[test]
    fn mutation_requires_a_later_successful_verification() {
        let mut gate = CompletionGate::default();
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        assert!(!gate.evidence().completed);

        gate.record(&outcome(2, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn inline_environment_introspection_does_not_unlock_completion() {
        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::Mutation, 1));
        let kind = classify_command(
            "python3 -c \"import sys; print('\\n'.join(sys.path))\"",
            30_000,
        );
        assert_eq!(kind, ToolKind::ReadOnly);
        gate.record(&outcome(2, kind, 0));
        assert!(!gate.evidence().completed);

        let version_kind = classify_command("python3 --version", 30_000);
        assert_eq!(version_kind, ToolKind::ReadOnly);
        gate.record(&outcome(3, version_kind, 0));
        assert!(!gate.evidence().completed);
    }

    #[test]
    fn project_runtime_and_background_service_are_classified_separately() {
        assert_eq!(
            classify_command("python3 smoke_test.py", 30_000),
            ToolKind::RuntimeProbe
        );
        assert_eq!(
            classify_command("python3 server.py >server.log 2>&1 &", 30_000),
            ToolKind::BackgroundServiceStart
        );
    }

    #[test]
    fn test_named_output_file_is_not_misclassified_as_a_test_command() {
        assert_eq!(
            classify_command(
                "cd /app && ./compressor > test.comp && ./decomp < test.comp",
                30_000,
            ),
            ToolKind::Mutation
        );
    }

    #[test]
    fn dependency_installation_does_not_count_as_verification() {
        let install_kind = classify_command("pip install pytest pytest-timeout", 60_000);
        assert_eq!(install_kind, ToolKind::Mutation);
        assert_eq!(
            classify_command("npm install && npm test", 120_000),
            ToolKind::Mutation
        );

        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::Verification, 1));
        gate.record(&outcome(2, install_kind, 0));
        assert!(!gate.evidence().completed);
        gate.record(&outcome(3, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn printf_section_headers_stay_read_only() {
        // Investigation batches print section headers with bare `printf` while
        // reading code. That is not a workspace mutation; classifying it as one
        // makes the completion gate demand delivery-grade verification for a
        // pure analysis turn (2026-07-16 session: seven repeated replies).
        for command in [
            "printf '== TasksColumn state ==\\n'; sed -n '485,690p' src/pages/Workspace/WorkspacePage.tsx",
            "set -e\nprintf '== enabled skills ==\\n'; grep -RIn 'enabled' src | head -20",
            "printf 'VERIFICATION_OK\\n'",
        ] {
            assert_eq!(
                classify_command(command, 300_000),
                ToolKind::ReadOnly,
                "{command}"
            );
        }
    }

    #[test]
    fn printf_with_redirect_is_still_a_mutation() {
        for command in ["printf 'x' > notes.txt", "printf 'line\\n' >> log.txt"] {
            assert_eq!(
                classify_command(command, 300_000),
                ToolKind::Mutation,
                "{command}"
            );
        }
    }

    #[test]
    fn frontend_verification_commands_count_as_verification() {
        // vitest / jest / tsc are the standard verification commands in a
        // TypeScript repo; the gate must accept them, not only `pnpm test`.
        for command in [
            "pnpm exec vitest run src/pages/Workspace/TaskCreator.test.tsx",
            "npx vitest run",
            "pnpm exec jest src/foo.test.ts",
            "pnpm exec tsc --noEmit",
            "pnpm exec tsc --noEmit && printf 'VERIFICATION_OK\\n'",
        ] {
            assert_eq!(
                classify_command(command, 300_000),
                ToolKind::Verification,
                "{command}"
            );
        }
    }

    #[test]
    fn read_only_failure_alone_does_not_demand_verification() {
        // Reading source that contains the literal string "error:" (semantic
        // failure detection) or a grep with no matches (exit 1) changes no
        // workspace state; the gate must not reject a final answer over it.
        let mut gate = CompletionGate::new(false);
        gate.record(&outcome(1, ToolKind::ReadOnly, 1));
        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);

        let mut gate = CompletionGate::new(false);
        let mut noisy_read = outcome(1, ToolKind::ReadOnly, 0);
        noisy_read.semantic_failure = true;
        gate.record(&noisy_read);
        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);
    }

    #[test]
    fn mutation_and_verification_failures_still_demand_verification() {
        let mut gate = CompletionGate::new(false);
        gate.record(&outcome(1, ToolKind::Mutation, 1));
        assert!(!gate.evidence().completed);
        gate.record(&outcome(2, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);

        let mut gate = CompletionGate::new(false);
        gate.record(&outcome(1, ToolKind::Verification, 1));
        assert!(!gate.evidence().completed);
        gate.record(&outcome(2, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn recovery_prompt_forbids_restating_the_previous_summary() {
        let gate = CompletionGate::new(true);
        let prompt = build_completion_recovery_prompt(&gate.evidence());
        assert!(prompt.contains("do not restate your previous summary"));
        assert!(prompt.contains("in the user's language"));
        assert!(prompt.contains("never mention internal mechanisms"));
    }

    #[test]
    fn ready_prompt_requires_user_language_and_bans_internal_terminology() {
        let prompt = build_completion_ready_prompt();
        assert!(prompt.contains("in the user's language"));
        assert!(prompt.contains("do not repeat analysis or summaries the user has already seen"));
        assert!(prompt.contains("Never mention internal mechanisms"));
    }

    #[test]
    fn build_and_install_commands_receive_the_available_long_timeout() {
        assert_eq!(
            effective_command_timeout_sec("pip install -e .", 60, 300),
            300
        );
        assert_eq!(
            effective_command_timeout_sec("python setup.py build_ext --inplace", 120, 300),
            300
        );
        assert_eq!(effective_command_timeout_sec("cargo test", 30, 300), 30);
        assert_eq!(effective_command_timeout_sec("npm install", 30, 120), 120);
    }

    #[test]
    fn desktop_default_allows_pure_read_only_or_no_tool_tasks() {
        let mut gate = CompletionGate::default();
        assert!(gate.evidence().completed);
        gate.record(&outcome(1, ToolKind::ReadOnly, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn headless_mode_requires_successful_action_evidence() {
        let mut gate = CompletionGate::new(true);
        assert!(!gate.evidence().completed);
        gate.record(&outcome(1, ToolKind::ReadOnly, 0));
        assert!(!gate.evidence().completed);
        gate.record(&outcome(2, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn failed_tool_relocks_gate_until_a_later_verification() {
        // A failed *execution* (runtime probe, verification, mutation) after a
        // green verification relocks the gate. A failed read must NOT — reads
        // change no state, and relocking on them turned analysis turns into
        // reject/re-answer loops (see read_only_failure_alone_...).
        let mut gate = CompletionGate::default();
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        gate.record(&outcome(2, ToolKind::Verification, 0));
        gate.record(&outcome(3, ToolKind::RuntimeProbe, 1));
        assert!(!gate.evidence().completed);

        gate.record(&outcome(4, ToolKind::Verification, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn failed_verification_cannot_be_cleared_by_adding_test_exclusions() {
        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut failed = outcome(2, ToolKind::Verification, 1);
        failed.command = "python -m pytest tests --ignore=tests/dead.py".to_owned();
        gate.record(&failed);

        gate.record(&outcome(3, ToolKind::RuntimeProbe, 0));
        assert!(!gate.evidence().completed);

        let mut narrowed = outcome(4, ToolKind::Verification, 0);
        narrowed.command =
            "python -m pytest tests --ignore=tests/dead.py -k 'not failing_test'".to_owned();
        gate.record(&narrowed);
        assert!(!gate.evidence().completed);
        assert!(gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("narrow")));

        let mut repaired = outcome(5, ToolKind::Verification, 0);
        repaired.command = "python -m pytest tests --ignore=tests/dead.py".to_owned();
        gate.record(&repaired);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn background_service_requires_bounded_functional_probe() {
        let mut gate = CompletionGate::default();
        let mut service = outcome(1, ToolKind::BackgroundServiceStart, 0);
        service.command = "nohup ./server >server.log 2>&1 & echo $! >server.pid".to_owned();
        gate.record(&service);
        gate.record(&outcome(2, ToolKind::FunctionalProbe { bounded: false }, 0));
        assert!(!gate.evidence().completed);

        gate.record(&outcome(3, ToolKind::FunctionalProbe { bounded: true }, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn background_service_requires_pid_and_log_evidence() {
        let mut gate = CompletionGate::default();
        let mut service = outcome(1, ToolKind::BackgroundServiceStart, 0);
        service.command = "nohup ./server &".to_owned();
        gate.record(&service);
        gate.record(&outcome(2, ToolKind::FunctionalProbe { bounded: true }, 0));

        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert!(evidence.blockers.iter().any(|item| item.contains("PID")));
        assert!(evidence.blockers.iter().any(|item| item.contains("log")));
    }

    #[test]
    fn semantic_failure_with_zero_exit_code_does_not_unlock_gate() {
        let mut failed = outcome(1, ToolKind::Verification, 0);
        failed.stdout = "3 tests failed".to_owned();
        failed = failed.with_detected_semantic_failure();
        assert!(!failed.succeeded());

        let mut gate = CompletionGate::default();
        gate.record(&failed);
        assert!(!gate.evidence().completed);

        let mut missing_dependency = outcome(2, ToolKind::Verification, 0);
        missing_dependency.stdout = "/usr/local/bin/python: No module named pytest".to_owned();
        missing_dependency = missing_dependency.with_detected_semantic_failure();
        assert!(!missing_dependency.succeeded());

        let mut piped_pytest = outcome(3, ToolKind::Verification, 0);
        piped_pytest.stdout =
            "FAILED tests/test_knot.py::test_invariants\n1 failed in 2.62s".to_owned();
        piped_pytest = piped_pytest.with_detected_semantic_failure();
        assert!(!piped_pytest.succeeded());

        let mut caught_component_error = outcome(4, ToolKind::Verification, 0);
        caught_component_error.stdout =
            "component functional check\nccomplexity error: invalid call signature\nchecks complete"
                .to_owned();
        caught_component_error = caught_component_error.with_detected_semantic_failure();
        assert!(!caught_component_error.succeeded());

        let mut mismatched_checksums = outcome(5, ToolKind::Verification, 0);
        mismatched_checksums.command =
            "./decomp < data.comp | md5sum && md5sum data.txt".to_owned();
        mismatched_checksums.stdout = concat!(
            "d41d8cd98f00b204e9800998ecf8427e  -\n",
            "4ae35d9160d5c74dd25a80cb0b4da870  data.txt\n"
        )
        .to_owned();
        mismatched_checksums = mismatched_checksums.with_detected_semantic_failure();
        assert!(!mismatched_checksums.succeeded());

        for output in [
            "E: Failed to fetch http://archive.example/package.deb",
            "gzip: stdin: invalid compressed data--format violated\ntar: Unexpected EOF in archive",
            "g++: internal compiler error: Segmentation fault signal terminated program cc1plus",
            "error: externally-managed-environment",
            "python3: can't open file '/workspace/setup.py': [Errno 2] No such file or directory",
        ] {
            let mut masked_failure = outcome(6, ToolKind::Verification, 0);
            masked_failure.stdout = output.to_owned();
            masked_failure = masked_failure.with_detected_semantic_failure();
            assert!(!masked_failure.succeeded(), "{output}");
        }
    }

    #[test]
    fn command_classification_requires_bounded_loopback_probe() {
        assert_eq!(
            classify_command("curl --max-time 2 http://localhost:3000/health", 5_000),
            ToolKind::FunctionalProbe { bounded: true }
        );
        assert_eq!(
            classify_command("curl http://localhost:3000/health", 5_000),
            ToolKind::FunctionalProbe { bounded: false }
        );
        assert_eq!(
            classify_command("nohup ./server >server.log 2>&1 &", 5_000),
            ToolKind::BackgroundServiceStart
        );
        assert_eq!(
            classify_command(
                "python -c \"import grpc; channel = grpc.insecure_channel('localhost:5328'); grpc.channel_ready_future(channel).result(timeout=2); stub.GetVal(req, timeout=2)\"",
                10_000,
            ),
            ToolKind::FunctionalProbe { bounded: true }
        );
    }

    #[test]
    fn completion_ready_prompt_tells_agent_to_converge() {
        let prompt = build_completion_ready_prompt();
        assert!(prompt.contains("evidence is satisfied"));
        assert!(prompt.contains("final response"));
    }

    #[test]
    fn budget_convergence_prompt_prioritizes_missing_delivery_stages() {
        let evidence = CompletionEvidence {
            blockers: vec!["at least one successful verification is required".to_owned()],
            ..CompletionEvidence::default()
        };

        let prompt = build_budget_convergence_prompt(3, &evidence);

        assert!(prompt.contains("3 model rounds remain"));
        assert!(prompt.contains("build, install, run outside the source directory"));
        assert!(prompt.contains("batch related reads, edits, and checks"));
        assert!(prompt.contains("compatible dependency version"));
        assert!(prompt.contains("local import aliases"));
        assert!(prompt.contains("PID, logs, bounded readiness"));
        assert!(prompt.contains("at least one successful verification is required"));
    }

    #[test]
    fn source_build_contract_requires_component_behavior_and_build_input_coverage() {
        let normalized_contract = EXECUTION_CONTRACT
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        assert!(normalized_contract.contains("every explicitly named required component"));
        assert!(normalized_contract.contains("actual build inputs"));
        assert!(normalized_contract.contains("rerun the same compatibility scan after editing"));
        assert!(normalized_contract.contains("unresolved matches remain"));
        assert!(normalized_contract.contains("local import aliases"));
        assert!(normalized_contract.contains(
            "importing or locating a compiled extension, plugin, or native library is not a functional check"
        ));

        let prompt = build_budget_convergence_prompt(3, &CompletionEvidence::default());
        assert!(prompt.contains("named component behavior"));
        assert!(prompt.contains("generated and compiled source inputs"));
    }

    #[test]
    fn long_runs_receive_an_early_and_late_convergence_checkpoint() {
        assert!(should_prompt_budget_convergence(16));
        assert!(should_prompt_budget_convergence(15));
        assert!(should_prompt_budget_convergence(8));
        assert!(should_prompt_budget_convergence(3));
        assert!(!should_prompt_budget_convergence(17));
        assert!(should_prompt_time_convergence(600, 900));
        assert!(should_prompt_time_convergence(120, 900));
        assert!(!should_prompt_time_convergence(601, 900));
    }

    #[test]
    fn finalization_budget_blocks_scope_expansion_until_source_scan_is_complete() {
        let evidence = CompletionEvidence {
            required_source_scan_extensions: vec![".py".to_owned(), ".pyx".to_owned()],
            source_evidence_paths: vec!["pkg/fast.pyx".to_owned(), "pkg/main.py".to_owned()],
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command(
            8,
            &evidence,
            "python -m pytest tests/test_unrelated.py -v",
            &ToolKind::Verification,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            8,
            &evidence,
            "grep -R 'np.int' --include='*.py' --include='*.pyx' . >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(!evaluate_budget_command_with_time(
            40,
            Some((240, 900)),
            &evidence,
            "python -m pytest tests/test_unrelated.py -v",
            &ToolKind::Verification,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            8,
            &evidence,
            "sed -i 's/np.int/np.int64/g' pkg/fast.pyx",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            8,
            &evidence,
            "read_file pkg/runtime.py",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(!evaluate_budget_command(
            8,
            &evidence,
            "read_file unrelated/other.py",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(!evaluate_budget_command(
            8,
            &evidence,
            "read_file .venv/lib/python/site-packages/other.py",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(!evaluate_budget_command(
            8,
            &evidence,
            "grep ^(import|from) pkg",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            8,
            &evidence,
            "rg '^(import|from) ' --glob '*.py' --glob '*.pyx' pkg",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
    }

    #[test]
    fn source_midpoint_budget_forces_install_before_more_exploration() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 12,
            last_mutation_sequence: Some(12),
            source_delivery_required: true,
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            60,
            Some((450, 900)),
            &evidence,
            "cat src/another_file.py",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            60,
            Some((450, 900)),
            &evidence,
            "python3 -m pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_delivery_checkpoint_starts_after_first_third_of_budget() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 8,
            last_source_mutation_sequence: Some(8),
            source_delivery_required: true,
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            64,
            Some((600, 900)),
            &evidence,
            "cat src/another_file.py",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
    }

    #[test]
    fn source_delivery_checkpoint_requires_install_after_successful_edit() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 10,
            last_mutation_sequence: Some(10),
            last_source_mutation_sequence: Some(10),
            source_delivery_required: true,
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            60,
            Some((590, 900)),
            &evidence,
            "sed -i 's/more/changes/g' src/second.py",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            60,
            Some((590, 900)),
            &evidence,
            "python3 -m pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_delivery_checkpoint_allows_one_repair_after_stage_failure() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 13,
            last_failure_sequence: Some(12),
            last_source_mutation_sequence: Some(10),
            source_delivery_required: true,
            ..CompletionEvidence::default()
        };

        assert!(evaluate_budget_command_with_time(
            58,
            Some((570, 900)),
            &evidence,
            "sed -i 's/broken/fixed/g' src/module.py",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_delivery_checkpoint_requires_runtime_after_install() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 11,
            last_source_mutation_sequence: Some(10),
            source_delivery_required: true,
            last_source_install_sequence: Some(11),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            57,
            Some((560, 900)),
            &evidence,
            "sed -i 's/more/changes/g' src/second.py",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            57,
            Some((560, 900)),
            &evidence,
            "cd /tmp && python3 -c 'import package'",
            &ToolKind::RuntimeProbe,
        )
        .is_allowed());
    }

    #[test]
    fn source_midpoint_budget_forces_external_runtime_after_install() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 13,
            last_mutation_sequence: Some(12),
            source_delivery_required: true,
            last_source_install_sequence: Some(13),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            59,
            Some((440, 900)),
            &evidence,
            "grep -R deprecated src",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            59,
            Some((440, 900)),
            &evidence,
            "cd /tmp && python3 -c 'import package'",
            &ToolKind::RuntimeProbe,
        )
        .is_allowed());
    }

    #[test]
    fn source_checkpoint_blocks_dependency_churn_after_project_install() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 13,
            source_delivery_required: true,
            last_source_install_sequence: Some(13),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            59,
            Some((440, 900)),
            &evidence,
            "python3 -m pip install another-dependency",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_checkpoint_allows_dependency_recovery_after_runtime_failure() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 14,
            last_failure_sequence: Some(14),
            source_delivery_required: true,
            last_source_install_sequence: Some(13),
            ..CompletionEvidence::default()
        };

        assert!(evaluate_budget_command_with_time(
            58,
            Some((430, 900)),
            &evidence,
            "python3 -m pip install missing-runtime-dependency",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_compatibility_scan_precedes_rebuild_at_midpoint() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 20,
            last_mutation_sequence: Some(20),
            last_source_mutation_sequence: Some(20),
            required_source_scan_extensions: vec![".py".to_owned(), ".pyx".to_owned()],
            source_delivery_required: true,
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan after the last source edit covering these build-input extensions: .py, .pyx; make the scan command return 0 only when no unresolved matches remain".to_owned(),
                "source-build delivery requires a successful install from source after the last source edit".to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        assert!(evaluate_budget_command_with_time(
            50,
            Some((430, 900)),
            &evidence,
            "grep -R deprecated --include='*.py' --include='*.pyx' . >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(!evaluate_budget_command_with_time(
            50,
            Some((430, 900)),
            &evidence,
            "python3 -m pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_midpoint_forces_project_tests_after_install_and_runtime() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 22,
            last_mutation_sequence: Some(20),
            last_source_mutation_sequence: Some(20),
            source_delivery_required: true,
            last_source_install_sequence: Some(21),
            last_external_source_runtime_sequence: Some(22),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            48,
            Some((420, 900)),
            &evidence,
            "grep -R TODO src",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            48,
            Some((420, 900)),
            &evidence,
            "python3 -m pytest tests -x -v",
            &ToolKind::Verification,
        )
        .is_allowed());
    }

    #[test]
    fn source_checkpoint_rejects_compound_delivery_stages() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 20,
            source_delivery_required: true,
            ..CompletionEvidence::default()
        };
        let command = "python3 -m pip install . && cd /tmp && python3 -c 'import package' && cd /workspace && python3 -m pytest";

        let decision = evaluate_budget_command_with_time(
            100,
            Some((890, 900)),
            &evidence,
            command,
            &ToolKind::Mutation,
        );

        assert!(!decision.is_allowed());
        assert!(matches!(
            decision,
            PolicyDecision::Deny { reason, .. }
                if reason.contains("one tool call per delivery stage")
        ));
    }

    #[test]
    fn source_install_detection_accepts_python_module_pip_with_options() {
        assert!(is_source_install_command_in(
            "cd /workspace && .venv/bin/python -m pip install --no-index --no-build-isolation --no-deps -e .",
            Some("/workspace"),
        ));
        assert!(is_source_install_command(
            ".venv/bin/pip3 install --no-deps --editable ./"
        ));
        assert!(!is_source_install_command(
            ".venv/bin/python -m pip install --no-index pytest"
        ));
        assert!(!is_source_install_command(
            "pip install --find-links . pytest"
        ));
        assert!(!is_source_install_command(
            "pip install -e ../other-project"
        ));
        assert!(!is_source_install_command(
            "rg --fixed-strings 'pip install .' docs"
        ));
        assert!(!is_source_install_command("printf 'x && pip install .'"));
        assert!(!is_source_install_command(
            "cd ../other-project && pip install ."
        ));
        assert!(!is_source_install_command_in(
            "cd ../other-project && pip install .",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command_in(
            "cd /other-project && pip install .",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command(
            "pip install --cache-dir . --help"
        ));
        assert!(!is_source_install_command_in(
            "../other-project/setup.py install",
            Some("/workspace"),
        ));
        assert!(is_source_install_command_in(
            "export PIP_NO_INDEX=1 && pip install .",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command_in(
            "source .venv/bin/activate && pip install .",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command_in(
            "cd /workspace/subproject && python -m pip install -q -e .",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command_in(
            "cd /workspace/linked-project && pip install .",
            Some("/workspace"),
        ));
        assert!(is_source_install_command("pip install -e."));
        assert!(is_source_install_command("pip install -qq -e ."));
        assert!(is_source_install_command(
            "pip install --proxy http://127.0.0.1:8080 -e ."
        ));
        assert!(is_source_install_command(
            "uv pip install --system --disable-pip-version-check ."
        ));
        assert!(!is_source_install_command_in(
            "python setup.py install --help",
            Some("/workspace"),
        ));
        assert!(!is_source_install_command_in(
            "npm install -g . --dry-run",
            Some("/workspace"),
        ));
        assert!(is_source_install_command(
            r".venv\Scripts\python.exe -m pip install ."
        ));
        assert!(is_source_install_command(
            r".venv\Scripts\pip.exe install ."
        ));
        assert!(is_source_install_command("py -m pip install ."));
        assert!(is_source_install_command("py -3 -m pip install ."));
        assert!(is_source_install_command("py -3.12 -m pip install ."));
        assert!(is_source_install_command(
            r#"& "C:\Program Files\Python\python.exe" -m pip install ."#
        ));
    }

    #[test]
    fn missing_test_runner_forces_dependency_recovery_before_source_edits() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 22,
            last_failure_sequence: Some(22),
            last_source_mutation_sequence: Some(20),
            source_delivery_required: true,
            last_source_install_sequence: Some(21),
            last_external_source_runtime_sequence: Some(22),
            last_project_test_sequence: Some(22),
            missing_test_runner: Some("pytest".to_owned()),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            47,
            Some((410, 900)),
            &evidence,
            "sed -i 's/old/new/g' src/module.py",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            47,
            Some((410, 900)),
            &evidence,
            "python3 -m pip install pytest",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn denied_source_mutation_does_not_invalidate_delivery_evidence() {
        let mut gate = CompletionGate::new_with_source_requirements(true, false, true, false);
        let mut edit = outcome(1, ToolKind::Mutation, 0);
        edit.command = "sed -i 's/old/new/g' src/module.py".to_owned();
        gate.record(&edit);

        let mut install = outcome(2, ToolKind::Mutation, 0);
        install.command = "python3 -m pip install -e .".to_owned();
        gate.record(&install);

        let mut runtime = outcome(3, ToolKind::RuntimeProbe, 0);
        runtime.command = "cd /tmp && python3 -c 'import package'".to_owned();
        gate.record(&runtime);

        let mut denied = outcome(4, ToolKind::Mutation, 0);
        denied.command = "sed -i 's/more/changes/g' src/module.py".to_owned();
        denied.return_code = None;
        denied.error = Some("policy denied command (execution_budget)".to_owned());
        gate.record(&denied);

        assert_eq!(gate.evidence().last_source_mutation_sequence, Some(1));
    }

    #[test]
    fn missing_test_runner_is_recorded_and_cleared_after_install() {
        let mut gate = CompletionGate::new(true);
        let mut tests = outcome(1, ToolKind::Verification, 0);
        tests.command = "python3 -m pytest tests -v".to_owned();
        tests.stdout = "/usr/local/bin/python3: No module named pytest".to_owned();
        tests = tests.with_detected_semantic_failure();
        gate.record(&tests);
        assert_eq!(
            gate.evidence().missing_test_runner.as_deref(),
            Some("pytest")
        );

        let mut install = outcome(2, ToolKind::Mutation, 0);
        install.command = "python3 -m pip install pytest".to_owned();
        gate.record(&install);
        assert_eq!(gate.evidence().missing_test_runner, None);
    }

    #[test]
    fn completion_summary_replaces_provider_tool_protocol_markup() {
        let text = "<｜｜DSML｜｜tool_calls><｜｜DSML｜｜invoke name=\"run_shell\">";

        assert_eq!(
            sanitize_completion_summary(text),
            "Implementation completed and post-change verification passed."
        );
        assert_eq!(
            sanitize_completion_summary("Changed parser; tests pass."),
            "Changed parser; tests pass."
        );
    }

    #[test]
    fn progress_tracker_prompts_after_bounded_read_only_exploration() {
        let mut tracker = ProgressTracker::new(3);
        assert!(tracker.record(&outcome(1, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker.record(&outcome(2, ToolKind::ReadOnly, 0)).is_none());
        let prompt = tracker.record(&outcome(3, ToolKind::ReadOnly, 0)).unwrap();
        assert!(prompt.contains("candidate implementation"));
        assert!(tracker.read_only_exhausted_before_mutation());
    }

    #[test]
    fn mutation_resets_read_only_progress_pressure() {
        let mut tracker = ProgressTracker::new(2);
        assert!(tracker.record(&outcome(1, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker.record(&outcome(2, ToolKind::Mutation, 0)).is_none());
        assert!(tracker.record(&outcome(3, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker.record(&outcome(4, ToolKind::ReadOnly, 0)).is_none());
    }

    #[test]
    fn runtime_probe_verifies_only_after_a_mutation() {
        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::RuntimeProbe, 0));
        assert!(!gate.evidence().completed);
        gate.record(&outcome(2, ToolKind::Mutation, 0));
        gate.record(&outcome(3, ToolKind::RuntimeProbe, 0));
        assert!(gate.evidence().completed);
    }

    #[test]
    fn source_compatibility_work_requires_clean_scan_for_compiled_inputs() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Update this source package for compatibility with the installed runtime.",
        );

        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/fast.pyx'])".to_owned();
        gate.record(&build_inputs);

        let mut source_edit = outcome(2, ToolKind::Mutation, 0);
        source_edit.command = "sed -i 's/old/new/' pkg/main.py".to_owned();
        gate.record(&source_edit);
        gate.record(&outcome(3, ToolKind::Verification, 0));

        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains(".pyx")));

        let mut incomplete_scan = outcome(4, ToolKind::ReadOnly, 0);
        incomplete_scan.command = "grep -R 'old' --include='*.py' pkg/ >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals".to_owned();
        gate.record(&incomplete_scan);
        assert!(!gate.evidence().completed);

        let mut complete_scan = outcome(5, ToolKind::ReadOnly, 0);
        complete_scan.command = "grep -R 'old' --include='*.py' --include='*.pyx' pkg/ >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals".to_owned();
        complete_scan.stdout = "PASSED: no unresolved compatibility matches".to_owned();
        gate.record(&complete_scan);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn failed_compound_source_edit_is_recorded_before_delivery_recovery() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Update this package for runtime compatibility, compile the extensions, install from source, and run the project tests.",
        );

        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/fast.pyx'])".to_owned();
        gate.record(&build_inputs);

        let mut mixed = outcome(2, ToolKind::Mutation, 1);
        mixed.command = "sed -i 's/legacy/modern/' pkg/main.py && python setup.py build_ext --inplace && pip install -e . --no-build-isolation && cd /tmp && python -c 'import pkg'".to_owned();
        mixed.stdout = "Runtime import failed after the source edit".to_owned();
        gate.record(&mixed);

        let evidence = gate.evidence();
        assert_eq!(evidence.last_source_mutation_sequence, Some(2));
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("clean repository-wide residual scan")));
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("install from source after the last source edit")));
    }

    #[test]
    fn failed_non_editing_mutation_does_not_record_a_source_edit() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Compile and install this source package, then run the project tests.",
        );
        let mut failed_build = outcome(1, ToolKind::Mutation, 1);
        failed_build.command = "python setup.py build_ext --inplace".to_owned();
        failed_build.stderr = "compiler failed".to_owned();

        gate.record(&failed_build);

        assert_eq!(gate.evidence().last_source_mutation_sequence, None);
    }

    #[test]
    fn chinese_source_compatibility_instruction_enables_delivery_gates() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "修复源码包的兼容性，构建编译扩展，从源码安装，在源码目录外验证，并运行完整项目测试。",
        );
        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/native.c'])".to_owned();
        gate.record(&build_inputs);

        let mut edit = outcome(2, ToolKind::Mutation, 0);
        edit.command = "sed -i 's/old_api/new_api/g' pkg/main.py".to_owned();
        gate.record(&edit);

        let evidence = gate.evidence();
        assert!(evidence.source_delivery_required);
        assert!(evidence.project_tests_required);
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("clean repository-wide residual scan")));
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("successful install from source")));
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("successful project tests")));
    }

    #[test]
    fn source_checkpoint_blocks_rebuild_until_alias_aware_residual_scan() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 4,
            last_mutation_sequence: Some(4),
            last_failure_sequence: None,
            required_source_scan_extensions: vec![".py".to_owned(), ".pyx".to_owned()],
            source_delivery_required: true,
            last_source_mutation_sequence: Some(4),
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            20,
            Some((500, 900)),
            &evidence,
            "python setup.py build_ext --inplace && pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());

        assert!(!evaluate_budget_command_with_time(
            20,
            Some((800, 900)),
            &evidence,
            "python setup.py build_ext --inplace && pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());

        assert!(evaluate_budget_command_with_time(
            20,
            Some((500, 900)),
            &evidence,
            "rg '^(import|from) ' --glob '*.py' --glob '*.pyx' .",
            &ToolKind::ReadOnly,
        )
        .is_allowed());

        assert!(evaluate_budget_command_with_time(
            20,
            Some((500, 900)),
            &evidence,
            "rg '^(import|from) ' --glob '*.py' --glob '*.pyx' . >/tmp/import-aliases; rg 'legacy_api' --glob '*.py' --glob '*.pyx' . >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals",
            &ToolKind::ReadOnly,
        )
        .is_allowed());

        assert!(!evaluate_budget_command_with_time(
            20,
            Some((500, 900)),
            &evidence,
            "sed -i 's/legacy_api/current_api/' src/module.py && python setup.py build_ext --inplace",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn source_checkpoint_rejects_fragile_no_match_command_substitution() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 4,
            last_mutation_sequence: Some(4),
            required_source_scan_extensions: vec![".py".to_owned(), ".pyx".to_owned()],
            source_delivery_required: true,
            last_source_mutation_sequence: Some(4),
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        let decision = evaluate_budget_command_with_time(
            20,
            Some((500, 900)),
            &evidence,
            "OUT=$(grep -R 'old_api' --include='*.py' --include='*.pyx' .) && if [ -z \"$OUT\" ]; then echo '0 unresolved'; exit 0; else exit 1; fi",
            &ToolKind::ReadOnly,
        );

        assert!(!decision.is_allowed());
        let PolicyDecision::Deny { reason, .. } = decision else {
            panic!("fragile residual scan should be denied");
        };
        assert!(reason.contains("temporary results file"));
        assert!(reason.contains("test ! -s"));
    }

    #[test]
    fn source_checkpoint_accepts_literal_nonzero_search_error_exit() {
        let extensions = BTreeSet::from([".py".to_owned()]);
        let command = "grep -rn 'old_value' --include='*.py' compatdemo tests >/tmp/residuals; grep_rc=$?; if [ \"$grep_rc\" -gt 1 ]; then exit 2; fi; test ! -s /tmp/residuals";

        assert!(is_repository_source_scan_command(command, &extensions));
    }

    #[test]
    fn source_checkpoint_rejects_masked_search_errors_and_unreachable_empty_check() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 4,
            required_source_scan_extensions: vec![".py".to_owned(), ".pyx".to_owned()],
            source_delivery_required: true,
            last_source_mutation_sequence: Some(4),
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        for command in [
            "OUT=$(command rg 'old_api' --glob '*.py' --glob '*.pyx' . || true); test ! -s /tmp/residuals",
            "rg 'old_api' --glob '*.py' --glob '*.pyx' . >/tmp/residuals && test ! -s /tmp/residuals",
            "rg 'old_api' --glob '*.py' --glob '*.pyx' . >/tmp/residuals; scan_status=$?; test ! -s /tmp/residuals",
        ] {
            assert!(
                !evaluate_budget_command_with_time(
                    20,
                    Some((500, 900)),
                    &evidence,
                    command,
                    &ToolKind::ReadOnly,
                )
                .is_allowed(),
                "unsafe residual scan was allowed: {command}"
            );
        }
    }

    #[test]
    fn failed_clean_residual_scan_records_shell_exit_recovery() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Repair this source package for runtime compatibility.",
        );
        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/fast.pyx'])".to_owned();
        gate.record(&build_inputs);

        let mut edit = outcome(2, ToolKind::Mutation, 0);
        edit.command = "sed -i 's/old_api/new_api/' pkg/main.py".to_owned();
        gate.record(&edit);

        let mut failed_scan = outcome(3, ToolKind::ReadOnly, 1);
        failed_scan.command = "OUT=$(grep -R 'old_api' --include='*.py' --include='*.pyx' .) && if [ -z \"$OUT\" ]; then echo 'SCAN CLEAN: 0 unresolved'; exit 0; else exit 1; fi".to_owned();
        failed_scan.stdout = "SCAN CLEAN: 0 unresolved".to_owned();
        gate.record(&failed_scan);

        let evidence = gate.evidence();
        assert!(evidence.blockers.iter().any(|blocker| {
            blocker.contains("reported zero residual matches but exited nonzero")
                && blocker.contains("test ! -s")
        }));
    }

    #[test]
    fn compound_edit_and_failed_clean_scan_records_shell_exit_recovery() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Repair this source package for runtime compatibility.",
        );
        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/fast.pyx'])".to_owned();
        gate.record(&build_inputs);

        let mut mixed = outcome(2, ToolKind::Mutation, 1);
        mixed.command = "sed -i 's/old_api/new_api/' pkg/main.py; OUT=$(rg 'old_api' --glob '*.py' --glob '*.pyx' .); echo 'SCAN CLEAN: 0 unresolved'".to_owned();
        mixed.stdout = "SCAN CLEAN: 0 unresolved".to_owned();
        gate.record(&mixed);

        assert!(gate.evidence().blockers.iter().any(|blocker| {
            blocker.contains("reported zero residual matches but exited nonzero")
        }));
    }

    #[test]
    fn convergence_prompt_requires_evidence_derived_variants_and_robust_scan_exit() {
        let evidence = CompletionEvidence {
            blockers: vec![
                "source compatibility work requires a clean repository-wide residual scan"
                    .to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        let prompt = build_time_convergence_prompt(300, &evidence);

        assert!(prompt.contains("exact failing API member"));
        assert!(prompt.contains("repository references or a language adapter"));
        assert!(!prompt.contains("direct and underscored spellings"));
        assert!(prompt.contains("temporary results file"));
        assert!(prompt.contains("reject status greater than 1"));
        assert!(prompt.contains("test ! -s"));
    }

    #[test]
    fn source_build_delivery_requires_install_and_runtime_outside_source_tree() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Compile the extensions and install this package from source, then make it usable.",
        );

        let mut build_inputs = outcome(1, ToolKind::ReadOnly, 0);
        build_inputs.command = "cat setup.py".to_owned();
        build_inputs.stdout = "Extension('pkg.fast', ['pkg/fast.pyx'])".to_owned();
        gate.record(&build_inputs);

        let mut edit = outcome(2, ToolKind::Mutation, 0);
        edit.command = "sed -i 's/old/new/' pkg/fast.pyx".to_owned();
        gate.record(&edit);

        let mut scan = outcome(3, ToolKind::ReadOnly, 0);
        scan.command = "grep -R old --include='*.pyx' . >/tmp/residuals; scan_status=$?; if [ \"$scan_status\" -gt 1 ]; then exit \"$scan_status\"; fi; test ! -s /tmp/residuals".to_owned();
        scan.stdout = "PASSED: source scan clean".to_owned();
        gate.record(&scan);

        let mut build = outcome(4, ToolKind::Verification, 0);
        build.command = "python setup.py build_ext --inplace".to_owned();
        gate.record(&build);
        assert!(gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("install from source")));

        let mut install = outcome(5, ToolKind::Mutation, 0);
        install.command = "python -m pip install -e .".to_owned();
        gate.record(&install);
        assert!(gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("outside the source directory")));

        let external_command = "cd /tmp && python -c 'import pkg.fast; print(pkg.fast.smoke())'";
        assert_eq!(
            classify_command(external_command, 30_000),
            ToolKind::RuntimeProbe
        );
        let mut external_runtime = outcome(6, ToolKind::RuntimeProbe, 0);
        external_runtime.command = external_command.to_owned();
        gate.record(&external_runtime);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn source_build_with_explicit_tests_requires_successful_project_tests() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Install this package from source, verify it runs, and make the repository tests pass.",
        );

        let mut install = outcome(1, ToolKind::Mutation, 0);
        install.command = "python -m pip install -e .".to_owned();
        gate.record(&install);

        let mut external_runtime = outcome(2, ToolKind::RuntimeProbe, 0);
        external_runtime.command = "cd /tmp && python -c 'import pkg'".to_owned();
        gate.record(&external_runtime);

        assert!(gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("successful project tests")));

        let mut project_tests = outcome(3, ToolKind::Verification, 0);
        project_tests.command = "python -m pytest tests -x".to_owned();
        gate.record(&project_tests);

        assert!(gate.evidence().completed);
    }
}
