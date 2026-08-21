use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
do not spend time restating an earlier draft. The next response must contain a bounded tool call \
that directly resolves a blocker unless a precise external blocker requires user action. After a failed mutation or check, \
use at most one bounded diagnostic read, then make the smallest corrective mutation or rerun a focused machine check; \
do not spend another response on text-only analysis. Reuse a successful local check when the workspace has not changed; \
do not rerun the same command merely to reconfirm it. Treat every rejected draft and this instruction as invisible to the user. \
Once the blockers are resolved, answer the user's original request directly in the user's language with a concise, \
self-contained result and relevant verification. Do not merely report the last command, refer to an unseen draft, or mention \
internal mechanisms such as this gate.",
        evidence.blockers.join("; ")
    )
}

/// Some reasoning transports accept tools but reject forced tool selection.
/// Keep this detector narrow so malformed requests and unrelated provider 400s
/// still fail visibly instead of silently changing request semantics.
pub fn provider_rejects_required_tool_choice(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("tool_choice")
        && (detail.contains("does not support")
            || detail.contains("not supported")
            || detail.contains("unsupported")
            || detail.contains("invalid"))
}

pub fn build_completion_ready_prompt() -> &'static str {
    "The structured completion evidence is satisfied as a candidate, but final acceptance still \
requires one coverage audit against the original request. Map every explicitly named behavior, \
component, artifact, and environment constraint to concrete evidence from this run. Import, file \
existence, or compilation alone does not prove named component behavior. If any required behavior \
lacks a functional check, use the available tools now to run the missing check and repair any \
failure. Request examples are smoke checks only for behavior over variable inputs; add a project \
test, dedicated verifier, generated/property check, or machine-checked non-example case. Review \
the repository diff and map every modified path to a requested change. When the \
original request limits the change scope, revert unrelated changes made by this run without \
touching pre-existing user changes, then rerun the relevant checks. If the request specifies a \
named tool, library, model, version, or revision, prove that the implementation exercised that exact named \
dependency instead of merely importing it. For a state-changing control path, capture \
before-and-after observable state and assert the requested effect rather than accepting a command \
acknowledgement. For source compatibility work, \
rerun a repository-wide residual search after the last \
source edit, cover every generated or compiled source suffix found in the build configuration, and \
make the command succeed only when no unresolved matches remain. If coverage is complete, stop and \
answer the user's original request directly with a concise, self-contained user-facing result in the \
user's language and include the relevant verification evidence. Treat internal drafts and review \
instructions as invisible: never refer to them or respond with only the most recent verification step. \
Never mention internal mechanisms (completion gate, coverage audit, candidate delivery) in the \
user-facing text; do not emit tool protocol markup, commands, or XML."
}

pub fn build_completion_summary_prompt() -> &'static str {
    "The structured completion evidence is already satisfied. Produce the final user-facing \
answer now using only evidence already collected in this run. Do not call tools, reread files, \
rerun checks, perform another coverage audit, or start new work. Summarize the concrete result and \
the relevant verification once, in the user's language. Treat internal drafts and this instruction \
as invisible. Never mention internal completion mechanisms or emit tool protocol markup."
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
stage is missing. Produce the first candidate required output artifact before the final third of \
the budget; do not spend that window on more research or dependency setup while the artifact is \
missing. During the convergence window, follow each successful mutation with a separate \
machine-checked verification that exits nonzero on mismatch before another edit or exploration. \
If that check only copies examples from the request, follow it with a project test, dedicated \
verifier, generated/property check, or at least one machine-checked non-example case before finalizing. \
To reduce model round trips, batch related reads and edits into one \
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

/// A narrow, model-actionable failure class for shell invocations. This is
/// deliberately orthogonal to [`ToolKind`]: an opaque CLI may be read-only,
/// but `command not found` still requires repair before the turn can finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandFailureKind {
    CommandNotFound,
    ResourceNotFound,
    InvalidInvocation,
    CommandTimeout,
    ShellUnavailable,
}

impl CommandFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandNotFound => "command_not_found",
            Self::ResourceNotFound => "resource_not_found",
            Self::InvalidInvocation => "invalid_invocation",
            Self::CommandTimeout => "command_timeout",
            Self::ShellUnavailable => "shell_unavailable",
        }
    }
}

/// Classify only failure shapes where checking the executable, resource, or
/// invocation can change the next attempt. A generic nonzero exit (including
/// grep/rg no-match) stays untyped so ordinary read-only exploration does not
/// create a false recovery obligation.
pub fn classify_command_failure(
    return_code: Option<i32>,
    output: &str,
) -> Option<CommandFailureKind> {
    let lower = output.to_ascii_lowercase();
    if return_code == Some(127)
        || lower.contains("command not found")
        || lower.contains("not recognized as an internal or external command")
        || (lower.contains("the term '") && lower.contains("is not recognized"))
    {
        return Some(CommandFailureKind::CommandNotFound);
    }
    if contains_any(
        &lower,
        &[
            "no such file or directory",
            "can't open file",
            "cannot open file",
            "the system cannot find the path specified",
            "cannot find path",
        ],
    ) {
        return Some(CommandFailureKind::ResourceNotFound);
    }
    if contains_any(
        &lower,
        &[
            "unexpected argument",
            "unrecognized argument",
            "unknown argument",
            "invalid argument",
            "unrecognized option",
            "unknown option",
            "invalid option",
        ],
    ) {
        return Some(CommandFailureKind::InvalidInvocation);
    }
    if contains_any(
        &lower,
        &[
            "failed to execute shell",
            "could not start shell",
            "shell executable was not found",
        ],
    ) {
        return Some(CommandFailureKind::ShellUnavailable);
    }
    None
}

/// Privacy-safe identity for comparing command attempts across provider and
/// process boundaries. The raw command remains in its existing tool-call
/// audit surface; recovery metadata stores only this digest.
pub fn command_fingerprint(command: &str, cwd: &str, timeout_secs: u64) -> String {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hasher.update([0]);
    hasher.update(cwd.as_bytes());
    hasher.update([0]);
    hasher.update(timeout_secs.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_long_running_observation_command(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "gh run watch",
            "gh pr checks",
            "gh run view",
            "gh pr view",
            "gh release view",
            "gh release download",
            "workflow_dispatch",
            "auto-release",
            "release.yml",
            "while gh ",
            "until gh ",
        ],
    ) || (contains_any(lower, &["sleep ", "for ", "while ", "until "])
        && contains_any(lower, &["gh run", "gh pr", "gh release", "github.com"]))
}

pub fn effective_command_timeout_sec(command: &str, requested: u64, maximum: u64) -> u64 {
    let maximum = maximum.max(1);
    let requested = requested.clamp(1, maximum);
    let lower = command.to_ascii_lowercase();
    if is_long_running_observation_command(&lower)
        || is_dependency_install_command(&lower)
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
    let shell_command = shell_control_text(command);
    let lower = shell_command.to_ascii_lowercase();
    let starts_background_service = contains_any(
        &lower,
        &[
            "nohup ",
            "docker run -d",
            "podman run -d",
            "systemctl start ",
            " -daemonize",
            " --daemon",
        ],
    ) || has_unquoted_background_operator(&lower);
    if starts_background_service {
        return ToolKind::BackgroundServiceStart;
    }
    if has_opaque_executable_heredoc(command) {
        return ToolKind::Mutation;
    }
    if is_delivery_state_mutation(&lower)
        || is_dependency_install_command(&lower)
        || has_workspace_mutation(&lower)
        || has_inline_interpreter_workspace_mutation(&lower)
    {
        return ToolKind::Mutation;
    }
    if is_delivery_verification(&lower) {
        return ToolKind::Verification;
    }
    if is_functional_probe(&lower) {
        return ToolKind::FunctionalProbe {
            bounded: timeout_ms > 0 && has_command_level_bound(&lower),
        };
    }
    let single_command_version_check = !contains_any(&lower, &["&&", ";", "\n"])
        && (lower.contains("--version") || shell_command.contains(" -V"))
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
    let shell_test_command = has_shell_test_command(&lower);
    let structured_shell_assertion = has_structured_shell_assertion(&lower);
    let case_assertion = has_case_assertion(&lower);
    // Read-only gh queries against the authoritative remote (run/PR/check
    // state) are verification — the third allowlist expansion (pnpm test →
    // vitest/tsc → gh). Mutating gh subcommands (merge, workflow run,
    // api -X POST) stay out of this lane.
    let gh_read_only_verification = ["gh run view", "gh run list", "gh pr checks", "gh pr view"]
        .iter()
        .any(|w| lower.contains(w))
        || (lower.contains("gh api")
            && !lower.contains(" -x ")
            && !lower.contains("--method")
            && !lower.contains(" -f "));
    if gh_read_only_verification
        && !lower.contains("gh pr merge")
        && !lower.contains("gh workflow run")
    {
        return ToolKind::Verification;
    }

    if shell_test_command
        || structured_shell_assertion
        || case_assertion
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
                "npm test",
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
    let inline_interpreter_snippet = is_inline_interpreter_snippet(&lower);
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

fn tokens_contain_delivery_mutation(words: &[String]) -> bool {
    words.windows(2).any(|pair| {
        pair[0] == "git" && matches!(pair[1].as_str(), "add" | "commit" | "push" | "tag")
    }) || words.windows(3).any(|triple| {
        triple[0] == "gh"
            && ((triple[1] == "pr"
                && matches!(triple[2].as_str(), "create" | "merge" | "close" | "reopen"))
                || (triple[1] == "workflow" && triple[2] == "run")
                || (triple[1] == "release"
                    && matches!(triple[2].as_str(), "create" | "edit" | "delete" | "upload")))
    })
}

fn delivery_command_words(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|word| {
            let command_word = word.rsplit("$(").next().unwrap_or(word);
            command_word
                .trim_matches(['\'', '"', '$', '(', ')', ';'])
                .to_ascii_lowercase()
        })
        .collect()
}

fn is_delivery_state_mutation(command: &str) -> bool {
    let command_substitution_mutates = command.contains("$(")
        && tokens_contain_delivery_mutation(&delivery_command_words(command));
    command_substitution_mutates
        || shell_verification_segments(command).iter().any(|segment| {
            tokens_contain_delivery_mutation(&delivery_command_words(&execution_payload(segment)))
        })
}

fn is_delivery_verification(command: &str) -> bool {
    shell_verification_segments(command).iter().any(|segment| {
        let words = execution_payload(segment)
            .split_whitespace()
            .map(|word| word.trim_matches(['\'', '"']).to_ascii_lowercase())
            .collect::<Vec<_>>();
        (words.starts_with(&["gh".to_owned(), "pr".to_owned(), "checks".to_owned()])
            && words.iter().any(|word| word == "--watch"))
            || (words.starts_with(&["gh".to_owned(), "run".to_owned(), "watch".to_owned()])
                && words.iter().any(|word| word == "--exit-status"))
    })
}

fn is_inline_interpreter_snippet(command: &str) -> bool {
    contains_any(
        command,
        &[
            "python -c ",
            "python3 -c ",
            "node -e ",
            "ruby -e ",
            "perl -e ",
        ],
    )
}

fn has_inline_interpreter_workspace_mutation(command: &str) -> bool {
    if !is_inline_interpreter_snippet(command) {
        return false;
    }

    let compact = command
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect::<String>();
    let opens_writable_file = compact.contains("open(")
        && contains_any(
            &compact,
            &[
                ",'w'", ",\"w\"", ",'a'", ",\"a\"", ",'x'", ",\"x\"", ",'w+", ",\"w+", ",'a+",
                ",\"a+",
            ],
        );

    opens_writable_file
        || contains_any(
            &compact,
            &[
                ".write_text(",
                ".write_bytes(",
                "writefilesync(",
                "writefile(",
                "appendfilesync(",
                "appendfile(",
                "fs.rename",
                "fs.unlink",
                "fs.rm",
                "fs.mkdir",
                "os.remove(",
                "os.unlink(",
                "os.rename(",
                "os.replace(",
                "os.mkdir(",
                "os.makedirs(",
                "shutil.",
                "file.write(",
                "fileutils.",
            ],
        )
}

fn has_workspace_mutation(command: &str) -> bool {
    // Bare `printf`/`echo` is often only a section heading. Redirecting it to
    // a workspace path still counts as mutation through the redirect check.
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
            "mkdir ",
            "rm ",
            "mv ",
            "cp ",
            "install ",
            "git apply",
            "git branch -m",
            "git branch -M",
            "git switch ",
            "git checkout ",
            "git fetch ",
            "git pull ",
            "git stash ",
        ],
    ) || has_non_transient_output_redirect(command)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeredocDelimiter {
    value: String,
    strip_tabs: bool,
    suppress_expansion: bool,
}

fn shell_control_text(command: &str) -> String {
    let mut control = String::with_capacity(command.len());
    let mut pending = VecDeque::<HeredocDelimiter>::new();
    let raw_lines = command.split_inclusive('\n').collect::<Vec<_>>();

    for (line_index, raw_line) in raw_lines.iter().copied().enumerate() {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(delimiter) = pending.front() {
            let candidate = if delimiter.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == delimiter.value {
                pending.pop_front();
            }
            continue;
        }

        let prior_control_len = control.len();
        control.push_str(raw_line);
        let Some(delimiters) = heredoc_delimiters(line) else {
            return "malformed-heredoc &".to_owned();
        };
        if is_literal_data_heredoc(line, &control[..prior_control_len], &delimiters)
            && heredoc_sequence_closes(&raw_lines, line_index + 1, &delimiters)
        {
            pending.extend(delimiters);
        }
    }

    control
}

fn has_opaque_executable_heredoc(command: &str) -> bool {
    let mut control = String::with_capacity(command.len());
    let mut pending = VecDeque::<HeredocDelimiter>::new();
    let raw_lines = command.split_inclusive('\n').collect::<Vec<_>>();

    for (line_index, raw_line) in raw_lines.iter().copied().enumerate() {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(delimiter) = pending.front() {
            let candidate = if delimiter.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == delimiter.value {
                pending.pop_front();
            }
            continue;
        }

        let Some(delimiters) = heredoc_delimiters(line) else {
            return true;
        };
        if delimiters.is_empty() {
            control.push_str(raw_line);
            continue;
        }
        if !heredoc_sequence_closes(&raw_lines, line_index + 1, &delimiters)
            || !is_literal_data_heredoc(line, &control, &delimiters)
        {
            return true;
        }
        control.push_str(raw_line);
        pending.extend(delimiters);
    }

    false
}

fn has_shell_test_command(command: &str) -> bool {
    command.split([';', '\n', '&', '|']).any(|segment| {
        let command = segment.trim_start();
        command == "test"
            || command.starts_with("test ")
            || command == "["
            || command.starts_with("[ ")
            || command == "[["
            || command.starts_with("[[ ")
            || command.starts_with("if [ ")
            || command.starts_with("if [[ ")
    })
}

fn has_structured_shell_assertion(command: &str) -> bool {
    ["fail", "rc", "status"].iter().any(|variable| {
        command.contains(&format!("{variable}=0"))
            && (command.contains(&format!("exit ${variable}"))
                || command.contains(&format!("exit \"${variable}\"")))
    }) && !contains_any(
        command,
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
    ) && !has_non_transient_output_redirect(command)
}

fn heredoc_delimiters(line: &str) -> Option<Vec<HeredocDelimiter>> {
    let bytes = line.as_bytes();
    let mut delimiters = Vec::new();
    let mut index = 0;
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    while index < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if byte == b'\\' && !single_quoted {
            escaped = true;
            index += 1;
            continue;
        }
        if byte == b'\'' && !double_quoted {
            single_quoted = !single_quoted;
            index += 1;
            continue;
        }
        if byte == b'"' && !single_quoted {
            double_quoted = !double_quoted;
            index += 1;
            continue;
        }
        if single_quoted || double_quoted {
            index += 1;
            continue;
        }
        if byte == b'#'
            && (index == 0
                || bytes[index - 1].is_ascii_whitespace()
                || matches!(bytes[index - 1], b';' | b'|' | b'&' | b'(' | b')'))
        {
            break;
        }
        if byte == b'<'
            && bytes.get(index + 1) == Some(&b'<')
            && is_inside_shell_arithmetic(&line[..index])
        {
            index += 2;
            continue;
        }
        if byte != b'<'
            || bytes.get(index + 1) != Some(&b'<')
            || bytes.get(index + 2) == Some(&b'<')
            || index
                .checked_sub(1)
                .is_some_and(|previous| bytes[previous] == b'<')
        {
            index += 1;
            continue;
        }

        let mut cursor = index + 2;
        let strip_tabs = bytes.get(cursor) == Some(&b'-');
        if strip_tabs {
            cursor += 1;
        }
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            break;
        }

        let quote = match bytes[cursor] {
            b'\'' | b'"' => Some(bytes[cursor]),
            _ => None,
        };
        let mut suppress_expansion = quote.is_some();
        if quote.is_some() {
            cursor += 1;
        }
        let start = cursor;
        let mut value = String::new();
        if let Some(quote) = quote {
            while bytes.get(cursor).is_some_and(|byte| *byte != quote) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&quote) {
                return None;
            }
            value.push_str(&line[start..cursor]);
        } else {
            while bytes.get(cursor).is_some_and(|byte| {
                !byte.is_ascii_whitespace()
                    && !matches!(byte, b';' | b'|' | b'&' | b'(' | b')' | b'<' | b'>')
            }) {
                if bytes[cursor] == b'\\' && bytes.get(cursor + 1).is_some() {
                    suppress_expansion = true;
                    cursor += 1;
                }
                value.push(bytes[cursor] as char);
                cursor += 1;
            }
        }
        if !value.is_empty() {
            delimiters.push(HeredocDelimiter {
                value,
                strip_tabs,
                suppress_expansion,
            });
        }
        index = cursor.saturating_add(usize::from(quote.is_some()));
    }

    Some(delimiters)
}

fn is_inside_shell_arithmetic(prefix: &str) -> bool {
    let Some(open) = prefix.rfind("((") else {
        return false;
    };
    match prefix.rfind("))") {
        Some(close) => close < open,
        None => true,
    }
}

fn is_literal_data_heredoc(
    line: &str,
    prior_control: &str,
    delimiters: &[HeredocDelimiter],
) -> bool {
    if delimiters.is_empty()
        || delimiters
            .iter()
            .any(|delimiter| !delimiter.suppress_expansion)
        || line.contains('|')
        || line.contains(">(")
        || line.contains("<(")
    {
        return false;
    }

    let Some(heredoc_start) = line.find("<<") else {
        return false;
    };
    let prefix = &line[..heredoc_start];
    let lower_prefix = prefix.to_ascii_lowercase();
    if contains_any(
        &lower_prefix,
        &[
            "alias cat=",
            "alias tee=",
            "function cat",
            "function tee",
            "cat()",
            "cat ()",
            "tee()",
            "tee ()",
        ],
    ) {
        return false;
    }
    let segment = prefix
        .rsplit_once("&&")
        .map_or(prefix, |(_, segment)| segment);
    let segment = segment
        .rsplit_once(';')
        .map_or(segment, |(_, segment)| segment)
        .trim();
    let command = segment.split_whitespace().next().unwrap_or("");
    matches!(
        command,
        "cat" | "/bin/cat" | "/usr/bin/cat" | "tee" | "/bin/tee" | "/usr/bin/tee"
    ) && !control_redefines_data_command(prior_control, command)
        && !control_redefines_data_command(prefix, command)
}

fn control_redefines_data_command(control: &str, command: &str) -> bool {
    if command.contains('/') {
        return false;
    }

    let function_compact = format!("{command}()");
    let function_spaced = format!("{command} ()");
    let function_keyword = format!("function {command}");
    let alias = format!("alias {command}=");
    let lower = control.to_ascii_lowercase();
    lower.contains(&function_compact)
        || lower.contains(&function_spaced)
        || lower.contains(&function_keyword)
        || lower.contains(&alias)
        || lower.contains("path=")
        || lower.contains("hash -p ")
}

fn heredoc_sequence_closes(
    raw_lines: &[&str],
    mut line_index: usize,
    delimiters: &[HeredocDelimiter],
) -> bool {
    for delimiter in delimiters {
        let mut found = false;
        while let Some(raw_line) = raw_lines.get(line_index) {
            line_index += 1;
            let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
            let line = line.strip_suffix('\r').unwrap_or(line);
            let candidate = if delimiter.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == delimiter.value {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

fn has_unquoted_background_operator(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut comment = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if comment {
            if byte == b'\n' {
                comment = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && !single_quoted {
            escaped = true;
            continue;
        }
        if byte == b'\'' && !double_quoted {
            single_quoted = !single_quoted;
            continue;
        }
        if byte == b'"' && !single_quoted {
            double_quoted = !double_quoted;
            continue;
        }
        if single_quoted || double_quoted {
            continue;
        }
        if byte == b'#'
            && (index == 0
                || bytes[index - 1].is_ascii_whitespace()
                || matches!(bytes[index - 1], b';' | b'|' | b'&' | b'(' | b')'))
        {
            comment = true;
            continue;
        }
        if byte != b'&' {
            continue;
        }

        let previous = index
            .checked_sub(1)
            .and_then(|position| bytes.get(position));
        let next = bytes.get(index + 1);
        let is_redirection_or_pipeline = previous
            .is_some_and(|value| matches!(value, b'&' | b'>' | b'<' | b'|'))
            || next.is_some_and(|value| matches!(value, b'&' | b'>'));
        if !is_redirection_or_pipeline {
            return true;
        }
    }
    false
}

fn has_non_transient_output_redirect(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && !single_quoted {
            escaped = true;
            continue;
        }
        if byte == b'\'' && !double_quoted {
            single_quoted = !single_quoted;
            continue;
        }
        if byte == b'"' && !single_quoted {
            double_quoted = !double_quoted;
            continue;
        }
        if single_quoted || double_quoted || byte != b'>' {
            continue;
        }

        let mut target_start = index + 1;
        if bytes.get(target_start) == Some(&b'>') {
            target_start += 1;
        }
        while bytes
            .get(target_start)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            target_start += 1;
        }
        if bytes.get(target_start) == Some(&b'&') {
            continue;
        }
        if bytes.get(target_start) == Some(&b'|') {
            target_start += 1;
            while bytes
                .get(target_start)
                .is_some_and(|byte| byte.is_ascii_whitespace())
            {
                target_start += 1;
            }
        }

        let target = command[target_start..]
            .split(|character: char| character.is_whitespace() || ";|&".contains(character))
            .next()
            .unwrap_or("")
            .trim_matches(['\'', '"'])
            .to_ascii_lowercase();
        if target.is_empty() {
            return true;
        }
        if target == "/dev/null"
            || target.starts_with("/tmp/")
            || target.starts_with("/var/tmp/")
            || target.starts_with("$tmp")
            || target.starts_with("${tmp")
        {
            continue;
        }
        return true;
    }
    false
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

fn has_machine_checked_assertion(command: &str) -> bool {
    if has_opaque_executable_heredoc(command) {
        return false;
    }
    let lower = shell_control_text(command).to_ascii_lowercase();
    let masks_assertion_failure = contains_any(&lower, &["|| true", "|| :"]);
    let contains_machine_check = is_project_test_command(&lower)
        || is_dedicated_verifier_command(&lower)
        || has_structured_shell_assertion(&lower)
        || has_case_assertion(&lower)
        || has_shell_test_command(&lower)
        || contains_machine_check_marker(&lower);
    contains_machine_check && !masks_assertion_failure && machine_check_controls_exit_status(&lower)
}

fn contains_machine_check_marker(command: &str) -> bool {
    contains_any(
        command,
        &[
            "assert ",
            "assert(",
            "assert_eq!",
            "assert_ne!",
            ".toequal(",
            ".tobe(",
            "cmp ",
            "diff ",
            "grep -q",
            "jq -e",
        ],
    )
}

fn machine_check_controls_exit_status(command: &str) -> bool {
    if has_structured_shell_assertion(command) || has_case_assertion(command) {
        return true;
    }

    if let Some(failure_branch) = command.rsplit_once("|| {").map(|(_, branch)| branch) {
        if has_nonzero_exit_or_return(failure_branch) {
            return true;
        }
    }
    if command.contains("if ")
        && command.contains("then")
        && command.contains("fi")
        && has_nonzero_exit_or_return(command)
    {
        return true;
    }

    command
        .split([';', '\n'])
        .rev()
        .map(str::trim)
        .find(|segment| !segment.is_empty())
        .is_some_and(|tail| {
            is_project_test_command(tail)
                || is_dedicated_verifier_command(tail)
                || has_shell_test_command(tail)
                || contains_machine_check_marker(tail)
        })
}

fn has_case_assertion(command: &str) -> bool {
    command.contains("case ")
        && command.contains(" in ")
        && command.contains("esac")
        && has_nonzero_exit_or_return(command)
}

fn has_nonzero_exit_or_return(command: &str) -> bool {
    contains_any(command, &["exit 1", "exit $", "return 1", "return $"])
}

fn is_dedicated_verifier_command(command: &str) -> bool {
    contains_any(
        command,
        &[
            "verify.py",
            "verify.sh",
            "verify.js",
            "verify.ts",
            "check.py",
            "check.sh",
            "check.js",
            "check.ts",
        ],
    )
}

fn instruction_requires_machine_checked_behavior(instruction: &str) -> bool {
    let lower = instruction.to_lowercase();
    contains_any(
        &lower,
        &[
            "should output",
            "must output",
            "expected output",
            "should print",
            "must print",
            "should return",
            "must return",
            "should give",
            "must give",
            " outputs ",
            " returns ",
            "应该输出",
            "应输出",
            "必须输出",
            "输出应为",
            "输出为",
            "应该返回",
            "应返回",
            "必须返回",
            "返回值",
            "期望输出",
        ],
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ExplicitExampleNumericLiterals {
    values: BTreeSet<String>,
    occurrences: usize,
    command_sources: BTreeSet<String>,
}

fn extract_explicit_example_numeric_literals(instruction: &str) -> ExplicitExampleNumericLiterals {
    const EXAMPLE_ANCHORS: [&str; 5] = ["for example", "as an example", "e.g.", "例如", "比如"];

    let lower = instruction.to_lowercase();
    let mut literals = ExplicitExampleNumericLiterals::default();
    for anchor in EXAMPLE_ANCHORS {
        let mut remainder = lower.as_str();
        while let Some(position) = remainder.find(anchor) {
            let example_start = position + anchor.len();
            let example = &remainder[example_start..];
            let example = example.split('\n').next().unwrap_or(example);
            let example = first_sentence(example);
            let example_literals = numeric_literal_occurrences(example);
            literals.occurrences += example_literals.len();
            literals.values.extend(example_literals);
            literals
                .command_sources
                .extend(example_command_sources(example));
            remainder = &remainder[example_start + example.len()..];
        }
    }
    literals
}

fn normalized_command_source(token: &str) -> Option<String> {
    let token = token
        .trim_matches(|character: char| {
            matches!(
                character,
                '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
            )
        })
        .trim_start_matches("./");
    let source = token.rsplit('/').next().unwrap_or(token);
    (!source.is_empty()
        && source.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        }))
    .then(|| source.to_ascii_lowercase())
}

fn example_command_sources(example: &str) -> BTreeSet<String> {
    const NON_COMMAND_ARGUMENT_LABELS: [&str; 20] = [
        "input",
        "value",
        "number",
        "integer",
        "case",
        "example",
        "argument",
        "arg",
        "parameter",
        "param",
        "with",
        "using",
        "for",
        "of",
        "输入",
        "值",
        "数字",
        "整数",
        "示例",
        "参数",
    ];
    let lower = example.to_lowercase();
    let input_clause_end = [
        " should output",
        " must output",
        " should print",
        " must print",
        " should return",
        " must return",
        " returns ",
        " outputs ",
        " 应该输出",
        " 应输出",
        " 必须输出",
        " 应该返回",
        " 应返回",
        " 必须返回",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .min()
    .unwrap_or(lower.len());
    let words = lower[..input_clause_end]
        .split_whitespace()
        .collect::<Vec<_>>();
    let Some(input_index) = words
        .iter()
        .rposition(|word| !numeric_literal_occurrences(word).is_empty())
    else {
        return BTreeSet::new();
    };
    words[..input_index]
        .iter()
        .rev()
        .filter(|word| !word.starts_with('-'))
        .filter_map(|word| normalized_command_source(word))
        .find(|source| !NON_COMMAND_ARGUMENT_LABELS.contains(&source.as_str()))
        .into_iter()
        .collect()
}

fn first_sentence(text: &str) -> &str {
    for (index, character) in text.char_indices() {
        if !matches!(character, '.' | '!' | '?' | '。' | '！' | '？') {
            continue;
        }
        if matches!(character, '。' | '！' | '？') {
            return &text[..index];
        }
        let end = index + character.len_utf8();
        if text[end..].chars().next().is_none_or(char::is_whitespace) {
            return &text[..index];
        }
    }
    text
}

fn numeric_literal_occurrences(text: &str) -> Vec<String> {
    semantic_words(text)
        .into_iter()
        .map(|token| token.trim_matches(['\'', '’', '`']).to_owned())
        .filter(|token| token.chars().all(|character| character.is_ascii_digit()))
        .filter_map(|token| token.parse::<u128>().ok())
        .map(|value| value.to_string())
        .collect()
}

fn shell_verification_segments(command: &str) -> Vec<String> {
    split_shell_syntax(&shell_control_text(command), false)
}

fn shell_pipeline_segments(statement: &str) -> Vec<String> {
    split_shell_syntax(statement, true)
}

fn split_shell_syntax(command: &str, split_pipes: bool) -> Vec<String> {
    fn finish(segments: &mut Vec<String>, current: &mut String) {
        let segment = current.trim();
        if !segment.is_empty() {
            segments.push(segment.to_owned());
        }
        current.clear();
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut characters = command.chars().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut backtick_quoted = false;
    let mut escaped = false;
    let mut paren_depth = 0_u32;

    while let Some(character) = characters.next() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && !single_quoted {
            current.push(character);
            escaped = true;
            continue;
        }
        if character == '\'' && !double_quoted && !backtick_quoted {
            single_quoted = !single_quoted;
            current.push(character);
            continue;
        }
        if character == '"' && !single_quoted && !backtick_quoted {
            double_quoted = !double_quoted;
            current.push(character);
            continue;
        }
        if character == '`' && !single_quoted {
            backtick_quoted = !backtick_quoted;
            current.push(character);
            continue;
        }
        if single_quoted || double_quoted || backtick_quoted {
            current.push(character);
            continue;
        }
        match character {
            '(' => {
                paren_depth += 1;
                current.push(character);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                current.push(character);
            }
            '#' if paren_depth == 0
                && current.chars().next_back().is_none_or(char::is_whitespace) =>
            {
                for comment in characters.by_ref() {
                    if comment == '\n' {
                        break;
                    }
                }
                finish(&mut segments, &mut current);
            }
            ';' | '\n' if paren_depth == 0 => finish(&mut segments, &mut current),
            '&' if paren_depth == 0 => {
                if characters.peek() == Some(&'&') {
                    characters.next();
                }
                finish(&mut segments, &mut current);
            }
            '|' if paren_depth == 0 => {
                let logical_or = characters.peek() == Some(&'|');
                if logical_or {
                    characters.next();
                }
                if logical_or || split_pipes {
                    finish(&mut segments, &mut current);
                } else {
                    current.push(character);
                }
            }
            _ => current.push(character),
        }
    }
    finish(&mut segments, &mut current);
    segments
}

fn segment_after_execution_prefixes(mut segment: &str) -> &str {
    loop {
        segment = segment.trim_start_matches(['(', '{', ' ']);
        let Some(first) = segment.split_whitespace().next() else {
            return "";
        };
        let assignment_prefix = first.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        });
        if assignment_prefix {
            segment = after_shell_word(segment);
            continue;
        }
        match first {
            "command" | "sudo" | "exec" | "env" => {
                segment = after_shell_word(segment);
            }
            "timeout" => {
                segment = after_shell_word(after_shell_word(segment));
            }
            _ => return segment,
        }
    }
}

fn after_shell_word(text: &str) -> &str {
    text.find(char::is_whitespace)
        .map(|index| text[index..].trim_start())
        .unwrap_or("")
}

fn execution_payload(segment: &str) -> String {
    let executable = segment_after_execution_prefixes(segment);
    let first = executable.split_whitespace().next().unwrap_or("");
    if matches!(first, "bash" | "sh" | "zsh") {
        let option_and_payload = after_shell_word(executable);
        let option = option_and_payload.split_whitespace().next().unwrap_or("");
        if option.starts_with('-') && option.contains('c') {
            return after_shell_word(option_and_payload)
                .trim_matches(['\'', '"'])
                .to_owned();
        }
    }
    executable.to_owned()
}

fn verification_is_nonexecuting(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase().replace(['\'', '"'], "");
    if normalized.contains('$') || normalized.contains('`') || normalized.contains('\\') {
        return true;
    }
    normalized.split_whitespace().any(|word| {
        matches!(
            word,
            "--no-run" | "--help" | "-h" | "--version" | "--co" | "-list" | "--markers"
        ) || word.starts_with("--collect")
            || word.starts_with("--list")
            || word.starts_with("--fixtures")
    })
}

fn invokes_project_test_command(command: &str) -> bool {
    shell_verification_segments(command)
        .iter()
        .any(|statement| {
            shell_pipeline_segments(statement).iter().any(|segment| {
                let lower = execution_payload(segment).to_ascii_lowercase();
                if verification_is_nonexecuting(&lower) {
                    return false;
                }
                contains_any_at_start(
                    &lower,
                    &[
                        "pytest",
                        "python -m pytest",
                        "python3 -m pytest",
                        "python -m unittest",
                        "python3 -m unittest",
                        "vitest",
                        "jest",
                        "cargo test",
                        "npm test",
                        "npm run test",
                        "pnpm test",
                        "pnpm exec vitest",
                        "yarn test",
                        "bun test",
                        "make test",
                        "ctest",
                        "go test",
                        "mvn test",
                        "gradle test",
                        "gradlew test",
                        "./gradlew test",
                        "dotnet test",
                        "swift test",
                    ],
                )
            })
        })
}

fn invokes_dedicated_verifier_command(command: &str) -> bool {
    shell_verification_segments(command)
        .iter()
        .any(|statement| {
            shell_pipeline_segments(statement).iter().any(|segment| {
                let executable = execution_payload(segment);
                if verification_is_nonexecuting(&executable) {
                    return false;
                }
                let words = executable.split_whitespace().collect::<Vec<_>>();
                let candidate = match words.first().copied() {
                    Some("python" | "python3" | "bash" | "sh" | "node") => words.get(1).copied(),
                    candidate => candidate,
                }
                .unwrap_or("")
                .trim_start_matches("./");
                matches!(
                    candidate.rsplit('/').next().unwrap_or(candidate),
                    "verify.py"
                        | "verify.sh"
                        | "verify.js"
                        | "verify.ts"
                        | "check.py"
                        | "check.sh"
                        | "check.js"
                        | "check.ts"
                )
            })
        })
}

fn contains_any_at_start(text: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        text == *prefix
            || text
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_whitespace))
    })
}

fn command_source_candidates(command: &str) -> BTreeSet<String> {
    let executable = execution_payload(command);
    let words = executable.split_whitespace().collect::<Vec<_>>();
    let mut candidates = BTreeSet::new();
    if let Some(source) = words
        .first()
        .and_then(|word| normalized_command_source(word))
    {
        candidates.insert(source);
    }
    if matches!(words.first().copied(), Some("python" | "python3" | "node")) {
        if let Some(source) = words
            .iter()
            .skip(1)
            .find(|word| !word.starts_with('-'))
            .and_then(|word| normalized_command_source(word))
        {
            candidates.insert(source);
        }
    }
    candidates
}

fn behavior_command_substitution(text: &str, expected_sources: &BTreeSet<String>) -> bool {
    fn substitution_end(text: &str, body_start: usize) -> Option<usize> {
        let bytes = text.as_bytes();
        let mut index = body_start;
        let mut depth = 1_u32;
        let mut single_quoted = false;
        let mut double_quoted = false;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if !single_quoted => index = (index + 2).min(bytes.len()),
                b'\'' if !double_quoted => {
                    single_quoted = !single_quoted;
                    index += 1;
                }
                b'"' if !single_quoted => {
                    double_quoted = !double_quoted;
                    index += 1;
                }
                b'$' if !single_quoted && !double_quoted && bytes.get(index + 1) == Some(&b'(') => {
                    depth += 1;
                    index += 2;
                }
                b')' if !single_quoted && !double_quoted => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        None
    }

    let bytes = text.as_bytes();
    let mut index = 0_usize;
    let mut single_quoted = false;
    let mut double_quoted = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if !single_quoted => index = (index + 2).min(bytes.len()),
            b'\'' if !double_quoted => {
                single_quoted = !single_quoted;
                index += 1;
            }
            b'"' if !single_quoted => {
                double_quoted = !double_quoted;
                index += 1;
            }
            b'$' if !single_quoted && bytes.get(index + 1) == Some(&b'(') => {
                let body_start = index + 2;
                let Some(end) = substitution_end(text, body_start) else {
                    return false;
                };
                if behavior_command_source(&text[body_start..end], expected_sources) {
                    return true;
                }
                index = end + 1;
            }
            b'`' if !single_quoted => {
                let body_start = index + 1;
                let mut end = body_start;
                while end < bytes.len() && bytes[end] != b'`' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                if end >= bytes.len() {
                    return false;
                }
                if behavior_command_source(&text[body_start..end], expected_sources) {
                    return true;
                }
                index = end + 1;
            }
            _ => index += 1,
        }
    }
    false
}

fn behavior_command_source(command: &str, expected_sources: &BTreeSet<String>) -> bool {
    if expected_sources.is_empty() {
        return false;
    }
    command_source_candidates(command)
        .iter()
        .any(|source| expected_sources.contains(source))
}

fn assignment_name(segment: &str) -> Option<String> {
    let (prefix, _) = segment.split_once('=')?;
    let name = prefix
        .split_whitespace()
        .next_back()
        .unwrap_or("")
        .trim_end_matches('+');
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some(name.to_owned())
}

fn assignment_name_and_literals(
    segment: &str,
    expected_sources: &BTreeSet<String>,
) -> Option<(String, Vec<String>)> {
    let name = assignment_name(segment)?;
    let (_, value) = segment.split_once('=')?;
    behavior_command_substitution(value, expected_sources)
        .then(|| (name, numeric_literal_occurrences(value)))
}

fn shell_variable_writes(statement: &str) -> Vec<String> {
    let payload = execution_payload(statement);
    let words = payload
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect::<Vec<_>>();
    match words.first().copied() {
        Some("printf") => words
            .windows(2)
            .find(|pair| pair[0] == "-v")
            .map(|pair| vec![pair[1].to_owned()])
            .unwrap_or_default(),
        Some("read" | "readarray" | "mapfile" | "unset") => words
            .iter()
            .skip(1)
            .filter(|word| !word.starts_with('-'))
            .map(|word| word.trim_end_matches('=').to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

fn transparent_pipeline_stage(statement: &str) -> bool {
    execution_payload(statement)
        .split_whitespace()
        .next()
        .is_some_and(|command| command.trim_start_matches("./") == "tee")
}

fn linked_verification_numeric_literals(
    command: &str,
    expected_sources: &BTreeSet<String>,
) -> Vec<String> {
    let statements = shell_verification_segments(command);
    let mut dynamic_assignments = BTreeMap::<String, Vec<String>>::new();
    let mut linked = Vec::new();

    for statement in &statements {
        let payload = execution_payload(statement);
        if contains_any_at_start(&payload, &["eval", "source", "."]) {
            dynamic_assignments.clear();
        }
        for name in shell_variable_writes(statement) {
            dynamic_assignments.remove(&name);
        }
        if let Some(name) = assignment_name(statement) {
            dynamic_assignments.remove(&name);
            if let Some((_, literals)) = assignment_name_and_literals(statement, expected_sources) {
                dynamic_assignments.insert(name, literals);
            }
        }
        let pipeline = shell_pipeline_segments(statement);
        for (index, assertion) in pipeline.iter().map(String::as_str).enumerate() {
            let assertion_payload = execution_payload(assertion);
            let lower = assertion_payload.to_ascii_lowercase();
            if !has_shell_test_command(&assertion_payload) && !contains_machine_check_marker(&lower)
            {
                continue;
            }
            let mut assertion_linked =
                behavior_command_substitution(&assertion_payload, expected_sources);
            if !assertion_linked && index > 0 {
                for producer in pipeline[..index].iter().rev() {
                    if behavior_command_source(producer, expected_sources) {
                        assertion_linked = true;
                        linked.extend(numeric_literal_occurrences(&execution_payload(producer)));
                        break;
                    }
                    if !transparent_pipeline_stage(producer) {
                        break;
                    }
                }
            }
            for (name, assignment_literals) in &dynamic_assignments {
                if assertion_payload.contains(&format!("${name}"))
                    || assertion_payload.contains(&format!("${{{name}}}"))
                {
                    assertion_linked = true;
                    linked.extend(assignment_literals.iter().cloned());
                }
            }
            if assertion_linked {
                linked.extend(numeric_literal_occurrences(&assertion_payload));
            }
        }
    }
    linked
}

fn verification_uses_only_explicit_examples(
    command: &str,
    explicit_example_numeric_literals: &BTreeSet<String>,
    explicit_example_command_sources: &BTreeSet<String>,
) -> bool {
    if explicit_example_numeric_literals.is_empty()
        || invokes_project_test_command(command)
        || invokes_dedicated_verifier_command(command)
    {
        return false;
    }

    let command_literals =
        linked_verification_numeric_literals(command, explicit_example_command_sources);
    let distinct_literals = command_literals.iter().cloned().collect::<BTreeSet<_>>();
    command_literals.len() < 2 || distinct_literals.is_subset(explicit_example_numeric_literals)
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
        self.semantic_failure =
            detect_semantic_failure_for_command(&self.command, &self.stdout, &self.stderr)
                || has_mismatched_checksums(&self.command, &self.stdout, &self.stderr);
        self
    }
}

fn detect_semantic_failure_for_command(command: &str, stdout: &str, stderr: &str) -> bool {
    let lower_command = command.to_ascii_lowercase();
    let expects_absence = contains_any(
        &lower_command,
        &[
            "test ! -e",
            "test ! -f",
            "test ! -d",
            "[ ! -e",
            "[ ! -f",
            "[ ! -d",
            "! kill -0",
            "! pgrep ",
        ],
    ) && !contains_any(&lower_command, &[";", "\n", "&&", "||", "|"]);
    if !expects_absence {
        return detect_semantic_failure(stdout, stderr);
    }

    let remaining_signals = format!("{stdout}\n{stderr}")
        .lines()
        .filter(|line| {
            let line = line.to_ascii_lowercase();
            !contains_any(
                &line,
                &[
                    "no such file or directory",
                    "process dead",
                    "process not running",
                ],
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    detect_semantic_failure(&remaining_signals, "")
}

pub fn detect_semantic_failure(stdout: &str, stderr: &str) -> bool {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    let explicit_error_line = combined.lines().any(|line| {
        let line = line.trim();
        let reports_error = line.starts_with("error:") || line.contains(" error:");
        let reports_failure = line.starts_with("failed:")
            || line.contains(" failed:")
            || line.starts_with("fail:")
            || line.contains(" fail:");
        let benign_summary = contains_any(
            line,
            &[
                "0 error",
                "no error",
                "without error",
                "error: none",
                "error: null",
                "is_error: false",
                "0 failed",
                "no failed",
                "failed: none",
                "failed: null",
                "failed: false",
            ],
        );
        (reports_error || reports_failure) && !benign_summary
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
                "no such file or directory",
                "process dead",
                "process not running",
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
    #[serde(default)]
    pub machine_checked_behavior_required: bool,
    #[serde(default)]
    pub last_machine_checked_verification_sequence: Option<u64>,
    #[serde(default)]
    pub verification_diversity_required: bool,
    #[serde(default)]
    pub last_example_only_verification_sequence: Option<u64>,
    #[serde(default)]
    pub last_independent_verification_sequence: Option<u64>,
    pub last_failure_sequence: Option<u64>,
    #[serde(default)]
    pub last_failure_diagnostic_sequence: Option<u64>,
    #[serde(default)]
    pub failed_verification_fingerprint: Option<String>,
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
    #[serde(default)]
    pub required_observable_states: Vec<String>,
    #[serde(default)]
    pub observed_observable_states: Vec<String>,
    #[serde(default)]
    pub delivery_completion_required: bool,
    #[serde(default)]
    pub delivery_completion_satisfied: bool,
    #[serde(default)]
    pub delivery_requested_ceiling: Option<String>,
    #[serde(default)]
    pub delivery_reached_ceiling: Option<String>,
    pub completed: bool,
    pub blockers: Vec<String>,
}

/// Returns true only when an executed tool materially advances the shared
/// completion evidence. Transport recovery limits must not be reset by failed
/// tools, diagnostic reads, or unrelated green checks.
pub fn completion_evidence_made_progress(
    before: &CompletionEvidence,
    after: &CompletionEvidence,
) -> bool {
    let advanced = |before: Option<u64>, after: Option<u64>| match (before, after) {
        (Some(before), Some(after)) => after > before,
        (None, Some(_)) => true,
        _ => false,
    };

    let verification_ticket_open = after.failed_verification_fingerprint.is_some();

    (!before.completed && after.completed)
        || after.blockers.len() < before.blockers.len()
        || (before.failed_verification_fingerprint.is_some()
            && after.failed_verification_fingerprint.is_none())
        || advanced(before.last_mutation_sequence, after.last_mutation_sequence)
        || advanced(
            before.last_successful_verification_sequence,
            after.last_successful_verification_sequence,
        )
        || (!verification_ticket_open
            && (advanced(
                before.last_machine_checked_verification_sequence,
                after.last_machine_checked_verification_sequence,
            ) || advanced(
                before.last_independent_verification_sequence,
                after.last_independent_verification_sequence,
            )))
        || advanced(
            before.last_service_pid_evidence_sequence,
            after.last_service_pid_evidence_sequence,
        )
        || advanced(
            before.last_service_log_evidence_sequence,
            after.last_service_log_evidence_sequence,
        )
        || (!verification_ticket_open
            && advanced(
                before.last_bounded_probe_sequence,
                after.last_bounded_probe_sequence,
            ))
        || advanced(
            before.last_source_scan_sequence,
            after.last_source_scan_sequence,
        )
        || advanced(
            before.last_source_mutation_sequence,
            after.last_source_mutation_sequence,
        )
        || advanced(
            before.last_source_install_sequence,
            after.last_source_install_sequence,
        )
        || advanced(
            before.last_external_source_runtime_sequence,
            after.last_external_source_runtime_sequence,
        )
        || (after.project_tests_required
            && !verification_ticket_open
            && advanced(
                before.last_successful_project_test_sequence,
                after.last_successful_project_test_sequence,
            ))
        || after.observed_observable_states.len() > before.observed_observable_states.len()
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
    let convergence_window = remaining_model_rounds <= 16
        || wall_time.is_some_and(|(remaining, total)| {
            total > 0 && remaining <= total.saturating_mul(2) / 3
        });
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

    let latest_successful_mutation_is_unverified = !evidence.source_delivery_required
        && evidence
            .last_mutation_sequence
            .is_some_and(|mutation| mutation == evidence.outcome_count)
        && evidence.last_failure_sequence.unwrap_or(0)
            < evidence.last_mutation_sequence.unwrap_or(0)
        && evidence
            .last_machine_checked_verification_sequence
            .unwrap_or(0)
            < evidence.last_mutation_sequence.unwrap_or(0);
    if convergence_window && latest_successful_mutation_is_unverified {
        let machine_checked_verification = matches!(
            kind,
            ToolKind::Verification
                | ToolKind::RuntimeProbe
                | ToolKind::FunctionalProbe { bounded: true }
        ) && has_machine_checked_assertion(command);
        if !machine_checked_verification {
            return deny(
                "fix_verification_loop",
                "the latest successful mutation is still unverified; run a separate machine-checked verification that exits nonzero on mismatch before another edit or exploration",
            );
        }
    }

    let verification_floor = evidence
        .last_mutation_sequence
        .into_iter()
        .chain(evidence.last_failure_sequence)
        .max()
        .unwrap_or(0);
    let independent_verification_pending = evidence.verification_diversity_required
        && evidence
            .last_example_only_verification_sequence
            .is_some_and(|sequence| sequence > verification_floor)
        && !evidence
            .last_independent_verification_sequence
            .is_some_and(|sequence| sequence > verification_floor);
    if convergence_window && independent_verification_pending {
        if matches!(kind, ToolKind::Mutation) {
            return PolicyDecision::Allow;
        }
        let machine_checked_verification = matches!(
            kind,
            ToolKind::Verification
                | ToolKind::RuntimeProbe
                | ToolKind::FunctionalProbe { bounded: true }
        ) && has_machine_checked_assertion(command);
        if !machine_checked_verification {
            return deny(
                "verification_diversity",
                "the latest check copied only request examples; create or run a focused test/verifier, generated/property check, or at least one machine-checked non-example case before further exploration",
            );
        }
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
        let explicit_source_repair_pending = evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("explicit source repair requires a source edit"));
        if explicit_source_repair_pending {
            let lower_command = command.to_ascii_lowercase();
            let repair_mutation = matches!(kind, ToolKind::Mutation)
                && !is_dependency_install_command(&lower_command)
                && !is_source_build_or_install_command(&lower_command);
            let failure_diagnostic = latest_tool_failed && matches!(kind, ToolKind::ReadOnly);
            if repair_mutation || failure_diagnostic {
                return PolicyDecision::Allow;
            }
            return deny(
                "execution_budget",
                "the source-delivery checkpoint has been reached and the requested source repair has not started; inspect only the latest concrete failure, then edit the relevant source before another build, install, or scope expansion",
            );
        }
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

    let unresolved_failure = evidence.last_failure_sequence.is_some_and(|failure| {
        failure > evidence.last_successful_verification_sequence.unwrap_or(0)
    });
    let failure_diagnostic_consumed = match (
        evidence.last_failure_sequence,
        evidence.last_failure_diagnostic_sequence,
    ) {
        (Some(failure), Some(diagnostic)) => diagnostic > failure,
        _ => false,
    };
    if (remaining_model_rounds <= 8 || time_finalization_window)
        && unresolved_failure
        && failure_diagnostic_consumed
        && matches!(kind, ToolKind::ReadOnly)
    {
        return deny(
            "failure_repair_loop",
            "the final-stage diagnostic read has already been used for the current failure; make the smallest corrective mutation or rerun a focused machine check before more read-only exploration",
        );
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationScope {
    family: String,
    working_directory: String,
    selectors: BTreeSet<String>,
    configuration: BTreeSet<String>,
    restrictions: BTreeSet<String>,
    exact_command: String,
}

impl VerificationScope {
    fn from_outcome(outcome: &ToolOutcome) -> Self {
        verification_scope(&outcome.command, outcome.working_directory.as_deref())
    }

    fn covers(&self, failed: &Self) -> bool {
        if self.family != failed.family
            || self.working_directory != failed.working_directory
            || self.configuration != failed.configuration
            || !self.restrictions.is_subset(&failed.restrictions)
        {
            return false;
        }
        if self.family == "exact" || self.family.starts_with("executable:") {
            return self.exact_command == failed.exact_command;
        }
        self.selectors.is_empty() || self.selectors.is_superset(&failed.selectors)
    }

    fn is_strictly_narrower_than(&self, failed: &Self) -> bool {
        self.family == failed.family
            && self.working_directory == failed.working_directory
            && self.configuration == failed.configuration
            && (self.restrictions.len() > failed.restrictions.len()
                || (failed.selectors.is_empty() && !self.selectors.is_empty()))
    }

    fn fingerprint(&self) -> String {
        let serialized = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            self.family,
            self.working_directory,
            self.selectors
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            self.configuration
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            self.restrictions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
            self.exact_command,
        );
        format!("{:x}", Sha256::digest(serialized.as_bytes()))
    }
}

#[derive(Debug, Clone)]
struct FailedVerification {
    scope: VerificationScope,
}

#[derive(Debug, Clone)]
pub struct CompletionGate {
    require_action: bool,
    outcome_count: u64,
    last_mutation_sequence: Option<u64>,
    last_successful_verification_sequence: Option<u64>,
    machine_checked_behavior_required: bool,
    last_machine_checked_verification_sequence: Option<u64>,
    verification_diversity_required: bool,
    explicit_example_numeric_literals: BTreeSet<String>,
    explicit_example_command_sources: BTreeSet<String>,
    last_example_only_verification_sequence: Option<u64>,
    last_independent_verification_sequence: Option<u64>,
    last_failure_sequence: Option<u64>,
    last_failure_diagnostic_sequence: Option<u64>,
    last_service_start_sequence: Option<u64>,
    last_service_pid_evidence_sequence: Option<u64>,
    last_service_log_evidence_sequence: Option<u64>,
    last_bounded_probe_sequence: Option<u64>,
    failed_verifications: Vec<FailedVerification>,
    scope_narrowing_sequence: Option<u64>,
    source_compatibility_audit_required: bool,
    source_change_required: bool,
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
    required_observable_states: BTreeSet<String>,
    observed_observable_states: BTreeMap<String, u64>,
    delivery_completion_required: bool,
    delivery_completion_satisfied: bool,
    delivery_requested_ceiling: Option<String>,
    delivery_reached_ceiling: Option<String>,
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
        let source_change_required = source_delivery_required
            && contains_any(
                &lower,
                &[
                    "modify",
                    "update",
                    "patch",
                    "change",
                    "repair the source",
                    "fix the source",
                    "source changes",
                    "修改源码",
                    "修复源码",
                    "源码改动",
                    "修改",
                    "修复",
                    "更新",
                ],
            );
        let mut gate = Self::new_with_source_requirements(
            require_action,
            source_compatibility_audit_required,
            source_delivery_required,
            project_tests_required,
        );
        gate.source_change_required = source_change_required;
        gate.required_observable_states = extract_expected_state_markers(instruction);
        gate.machine_checked_behavior_required =
            instruction_requires_machine_checked_behavior(instruction);
        let explicit_examples = extract_explicit_example_numeric_literals(instruction);
        gate.verification_diversity_required =
            gate.machine_checked_behavior_required && explicit_examples.occurrences >= 4;
        gate.explicit_example_numeric_literals = explicit_examples.values;
        gate.explicit_example_command_sources = explicit_examples.command_sources;
        gate
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
            machine_checked_behavior_required: false,
            last_machine_checked_verification_sequence: None,
            verification_diversity_required: false,
            explicit_example_numeric_literals: BTreeSet::new(),
            explicit_example_command_sources: BTreeSet::new(),
            last_example_only_verification_sequence: None,
            last_independent_verification_sequence: None,
            last_failure_sequence: None,
            last_failure_diagnostic_sequence: None,
            last_service_start_sequence: None,
            last_service_pid_evidence_sequence: None,
            last_service_log_evidence_sequence: None,
            last_bounded_probe_sequence: None,
            failed_verifications: Vec::new(),
            scope_narrowing_sequence: None,
            source_compatibility_audit_required,
            source_change_required: false,
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
            required_observable_states: BTreeSet::new(),
            observed_observable_states: BTreeMap::new(),
            delivery_completion_required: false,
            delivery_completion_satisfied: false,
            delivery_requested_ceiling: None,
            delivery_reached_ceiling: None,
        }
    }

    /// Record the structured result of the delivery state machine. Generic
    /// tool success is not business completion: only the arbiter may close the
    /// delivery requirement after requested/reached evidence agrees.
    pub fn record_delivery_completion(
        &mut self,
        requested_ceiling: impl Into<String>,
        reached_ceiling: impl Into<String>,
        satisfied: bool,
    ) {
        self.delivery_completion_required = true;
        self.delivery_completion_satisfied = satisfied;
        self.delivery_requested_ceiling = Some(requested_ceiling.into());
        self.delivery_reached_ceiling = Some(reached_ceiling.into());
    }

    pub fn record(&mut self, outcome: &ToolOutcome) {
        self.outcome_count += 1;
        if matches!(outcome.kind, ToolKind::ReadOnly) && self.has_unresolved_failure() {
            self.last_failure_diagnostic_sequence = Some(outcome.sequence);
        }
        let machine_checked_behavior = has_machine_checked_assertion(&outcome.command);
        let example_only_verification = self.verification_diversity_required
            && machine_checked_behavior
            && verification_uses_only_explicit_examples(
                &outcome.command,
                &self.explicit_example_numeric_literals,
                &self.explicit_example_command_sources,
            );
        let behavior_verification_satisfied =
            !self.machine_checked_behavior_required || machine_checked_behavior;
        if outcome.succeeded()
            && machine_checked_behavior
            && matches!(
                outcome.kind,
                ToolKind::Verification
                    | ToolKind::RuntimeProbe
                    | ToolKind::FunctionalProbe { bounded: true }
            )
        {
            self.last_machine_checked_verification_sequence = Some(outcome.sequence);
            if example_only_verification {
                self.last_example_only_verification_sequence = Some(outcome.sequence);
            } else {
                self.last_independent_verification_sequence = Some(outcome.sequence);
            }
        }
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
                if self.failed_verifications.is_empty() && behavior_verification_satisfied {
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
        if matches!(outcome.kind, ToolKind::Mutation) && outcome.succeeded() {
            self.last_mutation_sequence = Some(outcome.sequence);
        }
        if matches!(outcome.kind, ToolKind::BackgroundServiceStart) {
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
        let verification_like = matches!(
            outcome.kind,
            ToolKind::Verification
                | ToolKind::RuntimeProbe
                | ToolKind::FunctionalProbe { bounded: true }
        );
        let mut verification_scope_was_narrowed = false;
        if verification_like && outcome.succeeded() && behavior_verification_satisfied {
            let successful_scope = VerificationScope::from_outcome(outcome);
            verification_scope_was_narrowed = self
                .failed_verifications
                .iter()
                .any(|failed| successful_scope.is_strictly_narrower_than(&failed.scope));
            self.failed_verifications
                .retain(|failed| !successful_scope.covers(&failed.scope));
            if verification_scope_was_narrowed {
                self.scope_narrowing_sequence = Some(outcome.sequence);
            } else if self.failed_verifications.is_empty() {
                self.scope_narrowing_sequence = None;
            }
        } else if verification_like
            && !outcome.succeeded()
            && verification_reached_test_scope(outcome)
            && !is_inconclusive_remote_observation(outcome)
        {
            let failed_scope = VerificationScope::from_outcome(outcome);
            if !self
                .failed_verifications
                .iter()
                .any(|failed| failed.scope.covers(&failed_scope))
            {
                self.failed_verifications
                    .retain(|failed| !failed_scope.covers(&failed.scope));
                self.failed_verifications.push(FailedVerification {
                    scope: failed_scope,
                });
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
            if outcome.succeeded() && behavior_verification_satisfied {
                if self.failed_verifications.is_empty() && !verification_scope_was_narrowed {
                    self.last_successful_verification_sequence = Some(outcome.sequence);
                    self.scope_narrowing_sequence = None;
                }
            }
        }
        if matches!(outcome.kind, ToolKind::RuntimeProbe)
            && outcome.succeeded()
            && behavior_verification_satisfied
            && self.failed_verifications.is_empty()
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
            if self.failed_verifications.is_empty() && behavior_verification_satisfied {
                self.last_successful_verification_sequence = Some(outcome.sequence);
            }
        }
        if outcome.succeeded() {
            let observed = format!("{}\n{}", outcome.stdout, outcome.stderr);
            if matches!(
                outcome.kind,
                ToolKind::RuntimeProbe | ToolKind::FunctionalProbe { bounded: true }
            ) {
                for state in &self.required_observable_states {
                    if contains_observable_state(&observed, state) {
                        self.observed_observable_states
                            .insert(state.clone(), outcome.sequence);
                    }
                }
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
        if self.delivery_completion_required && !self.delivery_completion_satisfied {
            blockers.push(format!(
                "delivery completion arbitration is still open: reached {} but objective requires {}",
                self.delivery_reached_ceiling.as_deref().unwrap_or("unknown"),
                self.delivery_requested_ceiling.as_deref().unwrap_or("unknown")
            ));
        }
        if !self.failed_verifications.is_empty() {
            blockers.push(
                "rerun every unresolved failed check at the same or broader scope after the repair; unrelated or narrower green checks cannot close these failures"
                    .to_owned(),
            );
        }
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
            if self.machine_checked_behavior_required
                && !self
                    .last_machine_checked_verification_sequence
                    .is_some_and(|sequence| sequence > verification_floor)
            {
                blockers.push(
                    "the requested output or return value requires a later machine-checked assertion that exits nonzero on mismatch; printing expected and actual values is diagnostic evidence, not verification"
                        .to_owned(),
                );
            } else {
                match self.last_successful_verification_sequence {
                    Some(sequence) if sequence > verification_floor => {}
                    Some(_) => blockers.push(
                        "successful verification must be later than the last mutation or failed tool"
                            .to_owned(),
                    ),
                    None => blockers
                        .push("at least one successful verification is required".to_owned()),
                }
            }
            if self.verification_diversity_required
                && self
                    .last_machine_checked_verification_sequence
                    .is_some_and(|sequence| sequence > verification_floor)
                && !self
                    .last_independent_verification_sequence
                    .is_some_and(|sequence| sequence > verification_floor)
            {
                blockers.push(
                    "checks copied only from the request examples are smoke tests, not sufficient completion evidence; run a project test, dedicated verifier, generated/property check, or at least one machine-checked non-example case"
                        .to_owned(),
                );
            }
        }

        let observable_floor =
            verification_floor.max(self.last_service_start_sequence.unwrap_or(0));
        let missing_observable_states = self
            .required_observable_states
            .iter()
            .filter(|state| {
                !self
                    .observed_observable_states
                    .get(*state)
                    .is_some_and(|sequence| *sequence > observable_floor)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing_observable_states.is_empty() {
            blockers.push(format!(
                "a successful post-change runtime or bounded functional probe must observe each explicitly requested user-visible state: {}",
                missing_observable_states.join(", ")
            ));
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
            if self.source_change_required && self.last_source_mutation_sequence.is_none() {
                blockers.push(
                    "the explicit source repair requires a source edit before delivery".to_owned(),
                );
            }
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
            machine_checked_behavior_required: self.machine_checked_behavior_required,
            last_machine_checked_verification_sequence: self
                .last_machine_checked_verification_sequence,
            verification_diversity_required: self.verification_diversity_required,
            last_example_only_verification_sequence: self.last_example_only_verification_sequence,
            last_independent_verification_sequence: self.last_independent_verification_sequence,
            last_failure_sequence: self.last_failure_sequence,
            last_failure_diagnostic_sequence: self.last_failure_diagnostic_sequence,
            failed_verification_fingerprint: self
                .failed_verifications
                .first()
                .map(|failed| failed.scope.fingerprint()),
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
            required_observable_states: self.required_observable_states.iter().cloned().collect(),
            observed_observable_states: self
                .required_observable_states
                .iter()
                .filter(|state| {
                    self.observed_observable_states
                        .get(*state)
                        .is_some_and(|sequence| *sequence > observable_floor)
                })
                .cloned()
                .collect(),
            delivery_completion_required: self.delivery_completion_required,
            delivery_completion_satisfied: self.delivery_completion_satisfied,
            delivery_requested_ceiling: self.delivery_requested_ceiling.clone(),
            delivery_reached_ceiling: self.delivery_reached_ceiling.clone(),
            completed: blockers.is_empty(),
            blockers,
        }
    }

    fn has_unresolved_failure(&self) -> bool {
        self.last_failure_sequence.is_some_and(|failure| {
            failure > self.last_successful_verification_sequence.unwrap_or(0)
        })
    }
}

fn verification_scope(command: &str, working_directory: Option<&str>) -> VerificationScope {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let tokens = normalized
        .split(|character: char| {
            !(character.is_ascii_alphanumeric()
                || matches!(character, '_' | '-' | '.' | '/' | ':' | '@'))
        })
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let lower_tokens = tokens
        .iter()
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();

    let mut family = None;
    let mut runner_end = 0_usize;
    for (index, token) in lower_tokens.iter().enumerate() {
        let next = lower_tokens
            .get(index + 1)
            .map(String::as_str)
            .unwrap_or("");
        let next_next = lower_tokens
            .get(index + 2)
            .map(String::as_str)
            .unwrap_or("");
        let detected = match (token.as_str(), next, next_next) {
            ("cargo", "test", _) => Some(("cargo:test", index + 2)),
            ("go", "test", _) => Some(("go:test", index + 2)),
            ("dotnet", "test", _) => Some(("dotnet:test", index + 2)),
            ("swift", "test", _) => Some(("swift:test", index + 2)),
            ("gh", "run", "watch") => Some(("github:run-watch", index + 3)),
            ("gh", "pr", "checks") => Some(("github:pr-checks", index + 3)),
            ("python" | "python3", "-m", "pytest") => Some(("pytest", index + 3)),
            ("npm" | "pnpm" | "yarn" | "bun", "test", _) => Some(("javascript:test", index + 2)),
            ("npm" | "pnpm" | "yarn" | "bun", "exec", "vitest") => Some(("vitest", index + 3)),
            ("npm" | "pnpm" | "yarn" | "bun", "exec", "jest") => Some(("jest", index + 3)),
            ("pytest", _, _) => Some(("pytest", index + 1)),
            ("vitest", _, _) => Some(("vitest", index + 1)),
            ("jest", _, _) => Some(("jest", index + 1)),
            ("ctest", _, _) => Some(("ctest", index + 1)),
            ("mvn", "test", _) => Some(("maven:test", index + 2)),
            ("gradle" | "gradlew", "test", _) => Some(("gradle:test", index + 2)),
            _ => None,
        };
        if let Some((detected_family, end)) = detected {
            family = Some(detected_family.to_owned());
            runner_end = end;
            break;
        }
    }
    let structured_runner_detected = family.is_some();

    if family.is_none() {
        if let Some(executable) = tokens.iter().find(|token| {
            token.starts_with("./")
                || token.starts_with("../")
                || [".js", ".py", ".sh", ".ts"]
                    .iter()
                    .any(|extension| token.ends_with(extension))
        }) {
            family = Some(format!("executable:{executable}"));
        }
    }

    if has_machine_checked_assertion(command) && !structured_runner_detected {
        family = Some("exact".to_owned());
        runner_end = 0;
    }

    let mut effective_directory = PathBuf::from(working_directory.unwrap_or("."));
    for segment in shell_verification_segments(command) {
        let payload = execution_payload(&segment);
        let words = payload
            .split_whitespace()
            .map(|word| word.trim_matches(['\'', '"']))
            .collect::<Vec<_>>();
        if words.first().is_some_and(|word| *word == "cd") {
            if let Some(directory) = words.get(1) {
                let directory = Path::new(directory);
                effective_directory = if directory.is_absolute() {
                    directory.to_path_buf()
                } else {
                    effective_directory.join(directory)
                };
                effective_directory = normalize_lexical_path(&effective_directory);
            }
            continue;
        }
        if family
            .as_deref()
            .is_some_and(|family| segment_runs_verification_family(&payload, family))
        {
            break;
        }
    }
    let effective_directory = normalize_lexical_path(&effective_directory)
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_owned();

    let mut selectors = BTreeSet::new();
    let mut configuration = BTreeSet::new();
    let mut restrictions = BTreeSet::new();
    let mut consumed_value = false;
    let structured_runner = family
        .as_deref()
        .is_some_and(|family| family != "exact" && !family.starts_with("executable:"));
    for (index, token) in tokens
        .iter()
        .enumerate()
        .skip(runner_end)
        .filter(|_| structured_runner)
    {
        if consumed_value {
            consumed_value = false;
            continue;
        }
        let (raw_flag, inline_value) = token
            .split_once('=')
            .map(|(flag, value)| (flag, Some(value)))
            .unwrap_or((token, None));
        let flag = raw_flag.to_ascii_lowercase();
        let next_value = inline_value.or_else(|| tokens.get(index + 1).copied());
        let mut record_value = |target: &mut BTreeSet<String>| {
            if let Some(value) = next_value.filter(|value| !value.starts_with('-')) {
                target.insert(format!("{flag}:{value}"));
                consumed_value = inline_value.is_none();
            } else {
                target.insert(flag.clone());
            }
        };
        if matches!(
            family.as_deref(),
            Some("github:run-watch" | "github:pr-checks")
        ) {
            if index == runner_end {
                selectors.insert(format!("target:{token}"));
                continue;
            }
            if flag == "--repo" {
                record_value(&mut configuration);
            }
            // --interval/--watch/--exit-status affect polling mechanics only.
            continue;
        }
        if matches!(
            flag.as_str(),
            "-p" | "--package" | "--test" | "--bin" | "--example"
        ) {
            record_value(&mut selectors);
            continue;
        }
        if matches!(
            flag.as_str(),
            "-k" | "--filter" | "--grep" | "--skip" | "--exclude" | "--ignore" | "--ignore-glob"
        ) {
            record_value(&mut restrictions);
            continue;
        }
        if matches!(
            flag.as_str(),
            "--features"
                | "--all-features"
                | "--no-default-features"
                | "--workspace"
                | "--profile"
                | "--release"
                | "--target"
        ) {
            record_value(&mut configuration);
            continue;
        }
        let lower_token = token.to_ascii_lowercase();
        if family.is_some()
            && !token.starts_with('-')
            && !matches!(lower_token.as_str(), "run" | "test" | "tests")
            && (token.contains("::")
                || token.starts_with("./")
                || token.starts_with("../")
                || token.starts_with("tests/")
                || token.starts_with("src/")
                || matches!(
                    family.as_deref(),
                    Some("pytest" | "unittest" | "vitest" | "jest" | "ctest")
                )
                || index == runner_end)
        {
            selectors.insert(format!("target:{token}"));
        }
    }

    VerificationScope {
        family: family.unwrap_or_else(|| "exact".to_owned()),
        working_directory: effective_directory,
        selectors,
        configuration,
        restrictions,
        exact_command: normalized,
    }
}

fn segment_runs_verification_family(segment: &str, family: &str) -> bool {
    let words = segment
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric()
                    && !matches!(character, '_' | '-' | '.' | '/' | ':' | '@')
            })
            .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    let adjacent = |first: &str, second: &str| {
        words
            .windows(2)
            .any(|pair| pair[0] == first && pair[1] == second)
    };
    match family {
        "cargo:test" => adjacent("cargo", "test"),
        "go:test" => adjacent("go", "test"),
        "dotnet:test" => adjacent("dotnet", "test"),
        "swift:test" => adjacent("swift", "test"),
        "github:run-watch" => words
            .windows(3)
            .any(|triple| triple[0] == "gh" && triple[1] == "run" && triple[2] == "watch"),
        "github:pr-checks" => words
            .windows(3)
            .any(|triple| triple[0] == "gh" && triple[1] == "pr" && triple[2] == "checks"),
        "pytest" => {
            words.iter().any(|word| word == "pytest")
                || words
                    .windows(3)
                    .any(|triple| triple[1] == "-m" && triple[2] == "pytest")
        }
        "unittest" => words
            .windows(3)
            .any(|triple| triple[1] == "-m" && triple[2] == "unittest"),
        "javascript:test" => ["npm", "pnpm", "yarn", "bun"]
            .iter()
            .any(|runner| adjacent(runner, "test")),
        "vitest" | "jest" | "ctest" => words.iter().any(|word| word == family),
        "maven:test" => adjacent("mvn", "test"),
        "gradle:test" => adjacent("gradle", "test") || adjacent("gradlew", "test"),
        "exact" => true,
        executable if executable.starts_with("executable:") => executable
            .strip_prefix("executable:")
            .is_some_and(|target| words.iter().any(|word| word == target)),
        _ => false,
    }
}

fn shell_failed_before_verification_started(output: &str) -> bool {
    output.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        let shell_prefix = line.starts_with("zsh:")
            || line.starts_with("bash:")
            || line.starts_with("sh:")
            || line.starts_with("dash:");
        shell_prefix
            && (line.contains("read-only variable:")
                || line.contains("unbound variable")
                || line.contains("bad substitution")
                || line.contains("parse error near")
                || line.contains("syntax error near unexpected token"))
    })
}

/// A read-only remote observation that hit the tool's time cap before the
/// remote work finished.
///
/// It is a failed OBSERVATION, not a failed verification, and the difference
/// decides whether a turn can ever complete. Filing it as a failure opens a
/// ticket that every later success must "cover" by scope; a poll of the same
/// run with different flags does not always cover it, so the ticket outlives
/// the green pipeline that would have closed it. Measured on one 96-turn
/// session: 56 of the 197 tool failures inside recovery-exhausted turns were
/// timeouts, overwhelmingly CI polls against a release build that simply takes
/// longer than the cap.
///
/// Only observation commands qualify. A test suite that times out did run the
/// thing under test and hung — that stays a genuine failure.
fn is_inconclusive_remote_observation(outcome: &ToolOutcome) -> bool {
    let text = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    let timed_out = text.contains("command timed out") || text.contains("timed out after");
    timed_out && is_long_running_observation_command(&outcome.command.to_ascii_lowercase())
}

fn verification_reached_test_scope(outcome: &ToolOutcome) -> bool {
    let output = format!("{}\n{}", outcome.stdout, outcome.stderr).to_ascii_lowercase();
    !((output.contains("cd:") && output.contains("no such file or directory"))
        || output.contains("command not found")
        || shell_failed_before_verification_started(&output)
        || python_runner_module_missing(&outcome.command, &output)
        || output.contains("could not find `cargo.toml`")
        || output.contains("could not find cargo.toml"))
}

fn python_runner_module_missing(command: &str, output: &str) -> bool {
    let words = command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect::<Vec<_>>();
    let Some(module) = words
        .windows(2)
        .find_map(|pair| (pair[0] == "-m").then(|| pair[1].to_ascii_lowercase()))
    else {
        return false;
    };
    let missing = [
        format!("no module named {module}"),
        format!("no module named '{module}'"),
        format!("no module named \"{module}\""),
    ];
    !output.contains("traceback") && missing.iter().any(|pattern| output.contains(pattern))
}

fn extract_expected_state_markers(instruction: &str) -> BTreeSet<String> {
    let lower = instruction.to_lowercase();
    let mut markers = BTreeSet::new();
    for anchor in ["expect to see", "wait until"] {
        let mut remainder = lower.as_str();
        while let Some(position) = remainder.find(anchor) {
            let clause_prefix = remainder[..position]
                .rsplit(['.', ';', '\n'])
                .next()
                .unwrap_or("");
            let prefix_words = semantic_words(clause_prefix);
            if prefix_words
                .iter()
                .rev()
                .take(4)
                .any(|word| is_state_negation(word))
            {
                remainder = &remainder[position + anchor.len()..];
                continue;
            }
            let expected = &remainder[position + anchor.len()..];
            let expected = expected.split(['.', ';', '\n']).next().unwrap_or(expected);
            let tokens = semantic_words(expected);
            let mut content = Vec::new();
            let mut stopped_at_display_noun = false;
            for token in tokens.iter().map(String::as_str) {
                if matches!(token, "screen" | "page" | "prompt" | "message") && !content.is_empty()
                {
                    stopped_at_display_noun = true;
                    break;
                }
                if matches!(
                    token,
                    "the" | "a" | "an" | "is" | "visible" | "appears" | "shown" | "this" | "that"
                ) || token.len() < 4
                {
                    continue;
                }
                content.push(token);
            }
            let marker = if stopped_at_display_noun || content.len() == 1 {
                content.last().copied()
            } else {
                content
                    .iter()
                    .rev()
                    .copied()
                    .find(|token| matches!(*token, "ready" | "healthy" | "success"))
            };
            if let Some(marker) = marker {
                let negated = expected.find(marker).is_some_and(|start| {
                    observable_state_is_negated(expected, start, start + marker.len())
                });
                if !negated {
                    markers.insert(marker.to_owned());
                }
            }
            remainder = &remainder[position + anchor.len() + expected.len()..];
        }
    }
    markers
}

fn semantic_words(text: &str) -> Vec<String> {
    text.split(|character: char| {
        !(character.is_alphanumeric() || matches!(character, '_' | '-' | '\'' | '’'))
    })
    .filter(|word| !word.is_empty())
    .map(|word| word.replace('’', "'"))
    .collect()
}

fn contains_observable_state(output: &str, state: &str) -> bool {
    let lower = output.to_lowercase();
    lower.match_indices(state).any(|(start, _)| {
        let before = lower[..start].chars().next_back();
        if before.is_some_and(is_observable_state_character) {
            return false;
        }
        let suffix = &lower[start + state.len()..];
        !suffix
            .chars()
            .next()
            .is_some_and(is_observable_state_character)
            && !observable_state_is_negated(&lower, start, start + state.len())
    })
}

fn observable_state_is_negated(output: &str, start: usize, end: usize) -> bool {
    let clause_start = output[..start]
        .rfind(['.', ';', ',', '\n'])
        .map_or(0, |position| position + 1);
    let clause_end = output[end..]
        .find(['.', ';', ',', '\n'])
        .map_or(output.len(), |position| end + position);
    let before = semantic_words(&output[clause_start..start]);
    let after = semantic_words(&output[end..clause_end]);

    has_effective_state_negation(before.iter().chain(&after).map(String::as_str))
}

fn has_effective_state_negation<'a>(words: impl Iterator<Item = &'a str>) -> bool {
    let words = words.collect::<Vec<_>>();
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        if matches!(word, "no" | "without")
            && words
                .get(index + 1)
                .is_some_and(|next| is_guardrail_result_word(next))
        {
            index += 2;
            continue;
        }
        if is_state_negation(word) || is_negative_state_word(word) {
            return true;
        }
        index += 1;
    }
    false
}

fn is_state_negation(word: &str) -> bool {
    matches!(
        word,
        "no" | "not"
            | "never"
            | "without"
            | "missing"
            | "absent"
            | "unavailable"
            | "fail"
            | "failed"
            | "failure"
            | "cannot"
            | "can't"
            | "didn't"
    ) || word.ends_with("n't")
}

fn is_negative_state_word(word: &str) -> bool {
    matches!(
        word,
        "gone"
            | "hidden"
            | "removed"
            | "closed"
            | "stopped"
            | "down"
            | "offline"
            | "disappear"
            | "disappears"
            | "disappeared"
            | "vanish"
            | "vanishes"
            | "vanished"
    )
}

fn is_guardrail_result_word(word: &str) -> bool {
    matches!(
        word,
        "error" | "errors" | "failure" | "failures" | "warning" | "warnings" | "issue" | "issues"
    )
}

fn is_observable_state_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '-' | '_')
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
    if !has_source_content_mutation(&lower_command)
        || !command_mentions_source_input(&lower_command)
    {
        return false;
    }

    outcome.succeeded()
}

fn has_source_content_mutation(command: &str) -> bool {
    contains_any(
        command,
        &[
            "apply_patch",
            "edit_file ",
            "write_file ",
            "sed -i",
            "perl -pi",
            "tee ",
            "cat >",
            "cat >>",
            "write_text(",
            "git apply",
        ],
    ) || (contains_any(command, &["printf ", "echo "])
        && has_non_transient_output_redirect(&shell_control_text(command)))
}

fn command_mentions_source_input(command: &str) -> bool {
    let mut extensions = BTreeSet::new();
    collect_source_extensions(command, &mut extensions);
    !extensions.is_empty()
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
            ToolKind::ReadOnly | ToolKind::RuntimeProbe => {
                self.consecutive_read_only += 1;
                if self.consecutive_read_only >= self.read_only_limit && !self.read_only_exhausted {
                    self.read_only_exhausted = true;
                    return Some(if self.mutation_seen {
                        "The bounded post-change inspection budget is exhausted. Use the evidence \
already collected to make the smallest corrective edit or run a bounded functional verification \
now. Continue inspecting only if you can name a specific missing fact that blocks both actions."
                            .to_owned()
                    } else {
                        "The bounded inspection budget is exhausted and no implementation has \
started. Use the evidence already collected to create the smallest candidate implementation \
now, then run a real verification. Continue inspecting only if you can name a specific missing \
fact that blocks implementation."
                            .to_owned()
                    });
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

    pub fn read_only_exhausted(&self) -> bool {
        self.read_only_exhausted
    }

    pub fn mutation_seen(&self) -> bool {
        self.mutation_seen
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
        assert_eq!(
            classify_command("./server -daemonize -pidfile /tmp/server.pid", 30_000),
            ToolKind::BackgroundServiceStart
        );
        assert_eq!(
            classify_command("./worker --daemon --pidfile /tmp/worker.pid", 30_000),
            ToolKind::BackgroundServiceStart
        );
    }

    #[test]
    fn combined_background_service_start_and_probe_stays_a_service_start() {
        let command = concat!(
            "cd /var/www/html && python3 -m http.server 8080 &\n",
            "echo \"PID=$!\"\n",
            "sleep 1\n",
            "curl -s http://localhost:8080/"
        );

        assert_eq!(
            classify_command(command, 30_000),
            ToolKind::BackgroundServiceStart
        );

        assert_eq!(
            classify_command(
                "python3 server.py > service.log 2>&1 & echo $! > service.pid",
                30_000,
            ),
            ToolKind::BackgroundServiceStart
        );
        for command in [
            "printf 'x & y' > output.txt",
            "printf \"x & y\" > output.txt",
            "first && second",
            "command 2>&1",
            "command &> output.log",
            "command |& tee output.log",
            "# command &\nprintf done",
        ] {
            assert_ne!(
                classify_command(command, 30_000),
                ToolKind::BackgroundServiceStart,
                "{command}"
            );
        }
    }

    #[test]
    fn heredoc_source_body_does_not_start_a_background_service() {
        let command = concat!(
            "cat > encode.c <<'EOF'\n",
            "int encode(int *value) {\n",
            "    int *pointer = &value[0];\n",
            "    return *pointer > 0;\n",
            "}\n",
            "EOF\n",
            "gcc -c encode.c 2>&1\n",
        );

        assert_eq!(classify_command(command, 30_000), ToolKind::Mutation);
    }

    #[test]
    fn heredoc_payload_is_ignored_but_following_shell_backgrounding_is_preserved() {
        let source_only = concat!(
            "cat > worker.c <<-'EOF'\n",
            "\t// nohup ./fake & curl http://invalid > fake.log\n",
            "\tint *pointer = &value;\n",
            "\tEOF\n",
            "gcc -c worker.c\n",
        );
        assert_eq!(classify_command(source_only, 30_000), ToolKind::Mutation);

        let real_background = concat!(
            "cat > worker.c <<'EOF'\n",
            "int *pointer = &value;\n",
            "EOF\n",
            "./worker > worker.log 2>&1 &\n",
        );
        assert_eq!(
            classify_command(real_background, 30_000),
            ToolKind::BackgroundServiceStart
        );
    }

    #[test]
    fn executable_or_unclosed_heredocs_are_scanned_conservatively() {
        for command in [
            "bash <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "cat > source.c <<EOF\n$(./generator > generator.log 2>&1 &)\nEOF\n",
            "cat > source.c <<'EOF'\nint *pointer = &value;\n./server &\n",
            "cat > source.c <<'EOF\n./server > server.log 2>&1 &\nEOF\n",
            "tee >(bash) <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "cat <<'EOF' > >(bash)\n./server > server.log 2>&1 &\nEOF\n",
        ] {
            assert_eq!(
                classify_command(command, 30_000),
                ToolKind::BackgroundServiceStart,
                "{command}"
            );
        }

        assert_eq!(
            classify_command("value=$((1 << 2)); printf '%s\\n' \"$value\"", 30_000),
            ToolKind::ReadOnly
        );
        assert_eq!(
            classify_command("(( value = 1 << 2 )); printf '%s\\n' \"$value\"", 30_000),
            ToolKind::ReadOnly
        );
    }

    #[test]
    fn executable_interpreter_heredoc_cannot_masquerade_as_a_shell_test() {
        let command = concat!(
            "python3 <<'PY'\n",
            "test = 'diagnostic fixture'\n",
            "print('Expected: 42')\n",
            "PY\n",
        );

        assert_eq!(classify_command(command, 30_000), ToolKind::Mutation);
    }

    #[test]
    fn inline_interpreter_workspace_write_is_a_mutation() {
        let command = concat!(
            "python3 -c \"\n",
            "with open('cli.py','w') as f: f.write('print(42)')\n",
            "\" && cat cli.py",
        );

        assert_eq!(classify_command(command, 30_000), ToolKind::Mutation);
    }

    #[test]
    fn local_git_state_changes_are_mutations_in_review_only_turns() {
        for command in [
            "git branch -m feature/new-name",
            "git switch -c feature/new-name",
            "git checkout -b feature/new-name",
            "git fetch --prune origin main",
            "git stash push -u",
        ] {
            assert_eq!(
                classify_command(command, 30_000),
                ToolKind::Mutation,
                "{command:?} changes local repository state"
            );
        }
        assert_eq!(
            classify_command("git status --short --branch", 30_000),
            ToolKind::ReadOnly
        );
    }

    #[test]
    fn custom_or_redefined_data_commands_do_not_hide_executable_heredocs() {
        for command in [
            "./cat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "/tmp/tee output.txt <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "alias cat=bash; cat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "cat() { bash; }; cat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "cat() { bash; }\ncat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "alias tee=bash\ntee output.txt <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "PATH=./bin:$PATH\ncat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            "PATH=./bin:$PATH; cat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
            ":; cat() { bash; }\ncat <<'EOF'\n./server > server.log 2>&1 &\nEOF\n",
        ] {
            assert_eq!(
                classify_command(command, 30_000),
                ToolKind::BackgroundServiceStart,
                "{command}"
            );
        }
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
    fn explicit_user_visible_state_requires_semantic_probe_output() {
        let instruction = concat!(
            "Start the image so I can connect via telnet 127.0.0.1 6665. ",
            "When I run telnet I will expect to see the login prompt; I'll log in. ",
            "Start it in the background and block until it is ready."
        );
        let mut gate = CompletionGate::new_for_instruction(true, instruction);
        assert_eq!(
            gate.evidence().required_observable_states,
            vec!["login".to_owned()]
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut connected = outcome(2, ToolKind::FunctionalProbe { bounded: true }, 0);
        connected.stdout = "Connected to 127.0.0.1. Escape character is '^]'.".to_owned();
        gate.record(&connected);
        assert!(!gate.evidence().completed);

        let mut login = outcome(3, ToolKind::FunctionalProbe { bounded: true }, 0);
        login.stdout = "Welcome to Alpine Linux\nlocalhost login:".to_owned();
        gate.record(&login);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn negated_probe_output_does_not_observe_the_requested_state() {
        for output in [
            "Connected, but no login prompt observed",
            "login unavailable",
            "not ready",
            "failed to display login",
            "can't find login prompt",
            "didn't display login prompt",
            "login prompt is ready but not visible",
            "probe failed after three retries to display login prompt",
        ] {
            let mut gate = CompletionGate::new_for_instruction(
                true,
                "Start the console and wait until the login prompt is visible.",
            );
            gate.record(&outcome(1, ToolKind::Mutation, 0));
            let mut probe = outcome(2, ToolKind::FunctionalProbe { bounded: true }, 0);
            probe.stdout = output.to_owned();
            gate.record(&probe);

            assert!(!gate.evidence().completed, "{output}");
        }
    }

    #[test]
    fn affirmative_state_output_is_not_blocked_by_unrelated_negative_guardrails() {
        for output in [
            "login prompt is ready and no errors were reported",
            "login prompt ready without errors",
        ] {
            let mut gate = CompletionGate::new_for_instruction(
                true,
                "Start the console and wait until the login prompt is visible.",
            );
            gate.record(&outcome(1, ToolKind::Mutation, 0));
            let mut probe = outcome(2, ToolKind::FunctionalProbe { bounded: true }, 0);
            probe.stdout = output.to_owned();
            gate.record(&probe);

            assert!(gate.evidence().completed, "{output}");
        }
    }

    #[test]
    fn state_extraction_is_limited_to_affirmative_expect_or_wait_language() {
        for instruction in [
            "The current output contains error; fix the service.",
            "The response contains failure before the repair.",
            "I do not expect to see an error after the repair.",
            "The service should show the current error while diagnosing it.",
            "Wait until no login prompt remains.",
            "Wait until the login prompt is no longer visible.",
            "Wait until the login prompt disappears.",
            "Wait until the login prompt is gone.",
        ] {
            assert!(
                CompletionGate::new_for_instruction(false, instruction)
                    .evidence()
                    .required_observable_states
                    .is_empty(),
                "{instruction}"
            );
        }
    }

    #[test]
    fn read_only_text_does_not_satisfy_a_requested_user_visible_state() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Start the console and wait until the login prompt is visible.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut source_text = outcome(2, ToolKind::ReadOnly, 0);
        source_text.stdout = "documentation: login prompt".to_owned();
        gate.record(&source_text);
        gate.record(&outcome(3, ToolKind::Verification, 0));

        assert!(!gate.evidence().completed);
    }

    #[test]
    fn verification_test_names_do_not_satisfy_a_runtime_visible_state() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Start the console and wait until the login prompt is visible.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut tests = outcome(2, ToolKind::Verification, 0);
        tests.stdout = "login PASSED".to_owned();
        gate.record(&tests);

        assert!(!gate.evidence().completed);
    }

    #[test]
    fn expected_prompt_marker_survives_following_probe_wording() {
        let gate = CompletionGate::new_for_instruction(
            true,
            "I expect to see the login prompt through a bounded client probe.",
        );

        assert_eq!(
            gate.evidence().required_observable_states,
            vec!["login".to_owned()]
        );
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
    fn gh_read_only_queries_count_as_verification() {
        // Week-audit finding: the model verified a hotfix end-to-end with
        // eight `gh run view` / `gh pr checks` calls and the gate still
        // rejected its report as "unverified" — the allowlist's third bite
        // (pnpm test → vitest/tsc → gh). Read-only gh queries against the
        // authoritative remote ARE verification.
        for command in [
            "gh run view 29917625521 --repo BumStill/CodeFactory --json status",
            "gh run list --repo BumStill/CodeFactory --limit 5",
            "gh pr checks 166 --repo BumStill/CodeFactory",
            "gh pr view 166 --json statusCheckRollup",
            "gh api repos/BumStill/CodeFactory/commits/abc/check-runs",
        ] {
            assert_eq!(
                classify_command(command, 300_000),
                ToolKind::Verification,
                "{command}"
            );
        }
        // Mutating gh subcommands must NOT ride the verification lane.
        for command in [
            "gh pr merge 166 --squash",
            "gh workflow run auto-release.yml --ref main",
            "gh api repos/x/y/dispatches -X POST",
        ] {
            assert_ne!(
                classify_command(command, 300_000),
                ToolKind::Verification,
                "{command}"
            );
        }
    }

    #[test]
    fn frontend_verification_commands_count_as_verification() {
        // vitest / jest / tsc are the standard verification commands in a
        // TypeScript repo; the gate must accept them, not only `pnpm test`.
        for command in [
            "npm test",
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
    fn shell_assertion_with_temporary_output_counts_as_verification() {
        let command = concat!(
            "FAIL=0\n",
            "grep -R 'forbidden' . > /tmp/residual.txt || true\n",
            "if grep -q 'forbidden' /tmp/residual.txt; then FAIL=1; fi\n",
            "exit $FAIL"
        );

        assert_eq!(classify_command(command, 300_000), ToolKind::Verification);
    }

    #[test]
    fn explicit_expected_behavior_requires_a_machine_checked_probe() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Repair the CLI. Running `./tool 6` should output 42.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut printed_only = outcome(2, ToolKind::RuntimeProbe, 0);
        printed_only.command = "./tool 6".to_owned();
        printed_only.stdout = "0\nExpected: 42\n".to_owned();
        gate.record(&printed_only);
        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert!(
            evidence
                .blockers
                .iter()
                .any(|blocker| blocker.contains("machine-check")),
            "blockers: {:?}",
            evidence.blockers
        );

        let mut build_only = outcome(3, ToolKind::Verification, 0);
        build_only.command = "cargo build".to_owned();
        gate.record(&build_only);
        assert!(!gate.evidence().completed);

        let mut masked = outcome(4, ToolKind::Verification, 0);
        masked.command = "./tool 6 | grep -qx 42 || true".to_owned();
        gate.record(&masked);
        assert!(!gate.evidence().completed);

        let mut swallowed = outcome(5, ToolKind::Verification, 0);
        swallowed.command = "./tool 6 | grep -qx 42; echo done".to_owned();
        gate.record(&swallowed);
        assert!(!gate.evidence().completed);

        let mut asserted = outcome(6, ToolKind::Verification, 0);
        asserted.command = "actual=$(./tool 6); test \"$actual\" = 42".to_owned();
        asserted.stdout = "".to_owned();
        gate.record(&asserted);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn copied_instruction_examples_are_smoke_checks_not_completion_evidence() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Create a processor for arbitrary inputs. For example, `./tool 3` should output 9, and `./tool 5` should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut examples_only = outcome(2, ToolKind::Verification, 0);
        examples_only.command = concat!(
            "test \"$(./tool 3)\" = \"9\" && ",
            "test \"$(./tool 5)\" = \"25\""
        )
        .to_owned();
        gate.record(&examples_only);

        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert!(
            evidence
                .blockers
                .iter()
                .any(|blocker| blocker.contains("examples")),
            "blockers: {:?}",
            evidence.blockers
        );

        let mut existence_only = outcome(3, ToolKind::Verification, 0);
        existence_only.command = "test \"$(./tool 7)\"".to_owned();
        gate.record(&existence_only);
        assert!(!gate.evidence().completed);

        let mut independent_case = outcome(4, ToolKind::Verification, 0);
        independent_case.command = "test \"$(./tool 7)\" = \"49\"".to_owned();
        gate.record(&independent_case);
        assert!(
            gate.evidence().completed,
            "blockers: {:?}",
            gate.evidence().blockers
        );
    }

    #[test]
    fn operational_numbers_outside_assertions_do_not_fake_verification_diversity() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary inputs. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        for (sequence, command) in [
            (
                2,
                "echo 999 && test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25",
            ),
            (
                3,
                "timeout 30 sh -c 'test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25'",
            ),
        ] {
            let mut verification = outcome(sequence, ToolKind::Verification, 0);
            verification.command = command.to_owned();
            gate.record(&verification);
            assert!(
                !gate.evidence().completed,
                "unexpected completion for {command}"
            );
        }
    }

    #[test]
    fn example_literal_extraction_stops_before_later_operational_numbers() {
        let literals = extract_explicit_example_numeric_literals(
            "For example, ./tool 3 should output 9 and ./tool 5 should output 25. Use timeout 99 and version 2026 after verification.",
        );
        assert_eq!(
            literals.values,
            ["3", "5", "9", "25"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn example_literal_extraction_collects_repeated_example_sentences() {
        let literals = extract_explicit_example_numeric_literals(
            "For example, ./tool 3 should output 9. For example, ./tool 5 should output 25.",
        );
        assert_eq!(
            literals.values,
            ["3", "5", "9", "25"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn project_test_suite_supersedes_example_only_smoke_checks() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Implement behavior for arbitrary inputs. For example, tool 10 should output 100 and tool 20 should output 400.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut examples_only = outcome(2, ToolKind::Verification, 0);
        examples_only.command =
            "test \"$(./tool 10)\" = 100 && test \"$(./tool 20)\" = 400".to_owned();
        gate.record(&examples_only);
        assert!(!gate.evidence().completed);

        let mut project_tests = outcome(3, ToolKind::Verification, 0);
        project_tests.command = "cargo test".to_owned();
        gate.record(&project_tests);
        assert!(
            gate.evidence().completed,
            "blockers: {:?}",
            gate.evidence().blockers
        );
    }

    #[test]
    fn a_single_exact_example_does_not_force_diversity_for_fixed_output_work() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Repair this fixed command. For example, ./tool 6 should output 42.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut verification = outcome(2, ToolKind::Verification, 0);
        verification.command = "test \"$(./tool 6)\" = 42".to_owned();
        gate.record(&verification);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn r48_desktop_style_gate_requires_diversity_after_a_mutation() {
        let mut gate = CompletionGate::new_for_instruction(
            false,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut examples_only = outcome(2, ToolKind::Verification, 0);
        examples_only.command = "test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25".to_owned();
        gate.record(&examples_only);
        assert!(!gate.evidence().completed);
    }

    #[test]
    fn r48_constant_assertion_and_fuzz_comment_are_not_independent_evidence() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        for (sequence, command) in [
            (2, "test 999 = 999 # fuzz"),
            (3, "test 999 = 999 && test 998 = 998 # fuzz"),
            (4, "test 999 = 999\n# $(./tool 7) fuzz\ntest 998 = 998"),
            (5, "test \"$(printf '$(./tool 7)')\" = '$(./tool 7)'"),
        ] {
            let mut fake = outcome(sequence, ToolKind::Verification, 0);
            fake.command = command.to_owned();
            gate.record(&fake);
            assert!(!gate.evidence().completed, "unexpected: {command}");
        }
    }

    #[test]
    fn r48_unrelated_pipeline_and_quoted_test_name_do_not_bypass_diversity() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        for (sequence, command) in [
            (2, "printf ignored | test 999 -lt 1000"),
            (
                3,
                "echo 'cargo test'; test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25",
            ),
            (
                4,
                "printf 'ignored; cargo test --all' >/dev/null; test \"$(./tool 3)\" = 9 && test \"$(./tool 5)\" = 25",
            ),
        ] {
            let mut fake = outcome(sequence, ToolKind::Verification, 0);
            fake.command = command.to_owned();
            gate.record(&fake);
            assert!(!gate.evidence().completed, "unexpected: {command}");
        }
    }

    #[test]
    fn r48_static_reassignment_clears_the_dynamic_value_link() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "actual=$(./tool 7); export actual=999; test \"$actual\" = 999".to_owned();
        gate.record(&fake);
        assert!(!gate.evidence().completed);

        let mut builtin_overwrite = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        builtin_overwrite.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "actual=$(./tool 7); printf -v actual 49; test \"$actual\" = 49".to_owned();
        builtin_overwrite.record(&fake);
        assert!(!builtin_overwrite.evidence().completed);
    }

    #[test]
    fn r48_unrelated_executable_cannot_impersonate_the_example_target() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "expr 999 | grep -qx 999".to_owned();
        gate.record(&fake);
        assert!(!gate.evidence().completed);

        let mut chinese = CompletionGate::new_for_instruction(
            true,
            "处理任意整数。例如，工具输入 3 应输出 9，输入 5 应输出 25。",
        );
        chinese.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "expr 7 | grep -qx 49".to_owned();
        chinese.record(&fake);
        assert!(!chinese.evidence().completed);
    }

    #[test]
    fn r48_nonexecuting_test_and_verifier_modes_are_not_independent_evidence() {
        for command in [
            "cargo test --no-run",
            "cargo test --no'-run'",
            "cargo test --no\\-run",
            "flag=--no-run; cargo test \"$flag\"",
            "cargo test --version",
            "pytest --collect-only",
            "pytest -h",
            "vitest --list",
            "python3 verify.py --help",
        ] {
            let mut gate = CompletionGate::new_for_instruction(
                true,
                "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
            );
            gate.record(&outcome(1, ToolKind::Mutation, 0));
            let mut fake = outcome(2, ToolKind::Verification, 0);
            fake.command = command.to_owned();
            gate.record(&fake);
            assert!(!gate.evidence().completed, "unexpected: {command}");
        }
    }

    #[test]
    fn r48_variable_assignment_links_input_to_asserted_output() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut independent = outcome(2, ToolKind::Verification, 0);
        independent.command = "actual=$(./tool 7); test \"$actual\" = 49".to_owned();
        gate.record(&independent);
        assert!(
            gate.evidence().completed,
            "blockers: {:?}",
            gate.evidence().blockers
        );
    }

    #[test]
    fn r48_behavior_pipeline_links_input_to_grep_assertion() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut independent = outcome(2, ToolKind::Verification, 0);
        independent.command = "./tool 7 | grep -qx 49".to_owned();
        gate.record(&independent);
        assert!(
            gate.evidence().completed,
            "blockers: {:?}",
            gate.evidence().blockers
        );
    }

    #[test]
    fn r48_target_identity_comes_from_the_input_invocation_only() {
        let instruction = "Handle arbitrary values. For example, ./tool --input 3 should output 9 and ./tool --input 5 should output 25.";
        let mut fake_gate = CompletionGate::new_for_instruction(true, instruction);
        fake_gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "output 7 | grep -qx 49".to_owned();
        fake_gate.record(&fake);
        assert!(!fake_gate.evidence().completed);

        let mut discarded = CompletionGate::new_for_instruction(true, instruction);
        discarded.record(&outcome(1, ToolKind::Mutation, 0));
        let mut fake = outcome(2, ToolKind::Verification, 0);
        fake.command = "./tool --input 7 | printf 49 | grep -qx 49".to_owned();
        discarded.record(&fake);
        assert!(!discarded.evidence().completed);

        let mut real_gate = CompletionGate::new_for_instruction(true, instruction);
        real_gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut real = outcome(2, ToolKind::Verification, 0);
        real.command = "./tool --input 7 | grep -qx 49".to_owned();
        real_gate.record(&real);
        assert!(real_gate.evidence().completed);

        let phrased = extract_explicit_example_numeric_literals(
            "For example, run ./tool with input 3 should output 9 and input 5 should output 25.",
        );
        assert_eq!(phrased.command_sources, BTreeSet::from(["tool".to_owned()]));
    }

    #[test]
    fn r48_quoted_shell_target_is_linked_through_a_pipeline() {
        let instruction = "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.";
        let examples = extract_explicit_example_numeric_literals(instruction);
        assert_eq!(
            examples.command_sources,
            BTreeSet::from(["tool".to_owned()])
        );
        assert_eq!(
            shell_pipeline_segments("bash -lc './tool 7' | grep -qx 49"),
            vec!["bash -lc './tool 7'", "grep -qx 49"]
        );
        assert_eq!(
            shell_verification_segments("bash -lc './tool 7' | grep -qx 49"),
            vec!["bash -lc './tool 7' | grep -qx 49"]
        );
        assert_eq!(execution_payload("bash -lc './tool 7'"), "./tool 7");
        assert_eq!(
            command_source_candidates("bash -lc './tool 7'"),
            BTreeSet::from(["tool".to_owned()])
        );
        assert!(behavior_command_source(
            "bash -lc './tool 7'",
            &examples.command_sources,
        ));
        assert!(!behavior_command_substitution(
            "grep -qx 49",
            &examples.command_sources,
        ));
        assert_eq!(
            linked_verification_numeric_literals(
                "bash -lc './tool 7' | grep -qx 49",
                &examples.command_sources,
            ),
            vec!["7", "49"]
        );
        let mut gate = CompletionGate::new_for_instruction(true, instruction);
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut independent = outcome(2, ToolKind::Verification, 0);
        independent.command = "bash -lc './tool 7' | grep -qx 49".to_owned();
        gate.record(&independent);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn r48_quoted_test_runner_and_multistage_target_pipeline_are_supported() {
        let mut quoted = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        quoted.record(&outcome(1, ToolKind::Mutation, 0));
        let mut project_tests = outcome(2, ToolKind::Verification, 0);
        project_tests.command = "bash -lc 'cargo test'".to_owned();
        quoted.record(&project_tests);
        assert!(quoted.evidence().completed);

        let mut env_prefixed = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        env_prefixed.record(&outcome(1, ToolKind::Mutation, 0));
        let mut project_tests = outcome(2, ToolKind::Verification, 0);
        project_tests.command =
            "CARGO_TARGET_DIR=/tmp/cf cargo test -p codefactory-agent-core".to_owned();
        env_prefixed.record(&project_tests);
        assert!(env_prefixed.evidence().completed);

        let mut piped = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 3 should output 9 and ./tool 5 should output 25.",
        );
        piped.record(&outcome(1, ToolKind::Mutation, 0));
        let mut independent = outcome(2, ToolKind::Verification, 0);
        independent.command = "./tool 7 | tee /tmp/out | grep -qx 49".to_owned();
        piped.record(&independent);
        assert!(
            piped.evidence().completed,
            "blockers: {:?}",
            piped.evidence().blockers
        );
    }

    #[test]
    fn r48_example_anchor_is_not_double_counted() {
        let literals = extract_explicit_example_numeric_literals(
            "Handle one fixed case; e.g., ./tool 3 should output 9.",
        );
        assert_eq!(literals.occurrences, 2);
        let gate = CompletionGate::new_for_instruction(
            true,
            "Handle one fixed case; e.g., ./tool 3 should output 9.",
        );
        assert!(!gate.verification_diversity_required);
    }

    #[test]
    fn r48_zero_one_two_and_repeated_outputs_still_enable_diversity() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Handle arbitrary values. For example, ./tool 0 should output 1 and ./tool 2 should output 1.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut examples_only = outcome(2, ToolKind::Verification, 0);
        examples_only.command = "test \"$(./tool 0)\" = 1 && test \"$(./tool 2)\" = 1".to_owned();
        gate.record(&examples_only);
        assert!(!gate.evidence().completed);

        let mut independent = outcome(3, ToolKind::Verification, 0);
        independent.command = "test \"$(./tool 3)\" = 9".to_owned();
        gate.record(&independent);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn r48_chinese_sentence_boundary_excludes_following_operational_numbers() {
        let literals = extract_explicit_example_numeric_literals(
            "例如，工具输入 3 应输出 9，输入 5 应输出 25。超时 99 秒，版本 2026。",
        );
        assert_eq!(
            literals.values,
            ["3", "5", "9", "25"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn explicit_nonzero_if_branch_is_a_machine_checked_probe() {
        assert!(has_machine_checked_assertion(
            "output=$(./tool 6); if [ \"$output\" != 42 ]; then exit 1; fi"
        ));
        assert!(!has_machine_checked_assertion(
            "output=$(./tool 6); if [ \"$output\" != 42 ]; then echo mismatch; fi"
        ));
    }

    #[test]
    fn explicit_nonzero_case_fallback_is_a_machine_checked_probe() {
        let asserted = "output=$(./tool --version) && case \"$output\" in 'version=1.0.0') exit 0 ;; *) printf 'unexpected: %s\\n' \"$output\" >&2; exit 1 ;; esac";
        assert!(has_machine_checked_assertion(asserted));
        assert_eq!(classify_command(asserted, 300_000), ToolKind::Verification);
        assert!(!has_machine_checked_assertion(
            "output=$(./tool --version); case \"$output\" in 'version=1.0.0') echo ok ;; *) echo mismatch ;; esac"
        ));
    }

    #[test]
    fn dedicated_verifier_can_machine_check_explicit_behavior() {
        let mut gate = CompletionGate::new_for_instruction(
            true,
            "Repair the processor; summarize(values) should return sorted squares.",
        );
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        let mut verifier = outcome(2, ToolKind::RuntimeProbe, 0);
        verifier.command = "python3 verify.py".to_owned();
        verifier.stdout = "PUBLIC_ACCEPTANCE_OK\n".to_owned();
        gate.record(&verifier);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn structured_shell_assertion_cannot_hide_a_workspace_write() {
        for command in [
            "FAIL=0; printf x > src/file.rs; exit $FAIL",
            "FAIL=0; printf x >src/file.rs; exit $FAIL",
            "STATUS=0; echo x 1>>src/file.rs; exit $STATUS",
            "RC=0; echo x 2>src/error.log; exit $RC",
        ] {
            assert_eq!(
                classify_command(command, 300_000),
                ToolKind::Mutation,
                "{command}"
            );
        }

        let temporary = "FAIL=0; grep forbidden . >/tmp/residual.txt || true; exit $FAIL";
        assert_eq!(classify_command(temporary, 300_000), ToolKind::Verification);
    }

    #[test]
    fn runtime_assertion_cannot_hide_a_workspace_write() {
        let command = concat!(
            "cat > cli.py <<'EOF'\n",
            "print(42)\n",
            "EOF\n",
            "output=$(python3 cli.py)\n",
            "[ \"$output\" = \"42\" ] || { exit 1; }\n",
        );

        assert_eq!(classify_command(command, 300_000), ToolKind::Mutation);
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
    fn classifies_only_actionable_command_failures() {
        let cases = [
            (
                Some(127),
                "zsh: command not found: old-skill-cli",
                Some(CommandFailureKind::CommandNotFound),
            ),
            (
                Some(1),
                "python3: can't open file 'scripts/run.py': No such file or directory",
                Some(CommandFailureKind::ResourceNotFound),
            ),
            (
                Some(2),
                "error: unexpected argument '--verison' found",
                Some(CommandFailureKind::InvalidInvocation),
            ),
            (Some(1), "grep: no matches", None),
            (Some(1), "business check returned false", None),
        ];

        for (return_code, text, expected) in cases {
            assert_eq!(
                classify_command_failure(return_code, text),
                expected,
                "unexpected classification for {text:?}",
            );
        }
    }

    #[test]
    fn mutation_and_verification_failures_still_demand_verification() {
        let mut gate = CompletionGate::new(false);
        gate.record(&outcome(1, ToolKind::Mutation, 1));
        assert_eq!(gate.evidence().last_mutation_sequence, None);
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
    fn unrelated_green_check_does_not_resolve_failed_check() {
        let mut gate = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test worker::tests::original_behavior".to_owned();
        gate.record(&failed);

        let mut unrelated = outcome(2, ToolKind::Verification, 0);
        unrelated.command = "cargo test parser::tests::unrelated_behavior".to_owned();
        gate.record(&unrelated);

        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert!(evidence.failed_verification_fingerprint.is_some());
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("every unresolved failed check")));

        let mut repaired = outcome(3, ToolKind::Verification, 0);
        repaired.command = "cargo test worker::tests::original_behavior".to_owned();
        gate.record(&repaired);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn narrower_failed_check_cannot_replace_the_original_failure() {
        let mut gate = CompletionGate::new(true);
        let mut workspace_failure = outcome(1, ToolKind::Verification, 1);
        workspace_failure.command = "cargo test --workspace".to_owned();
        workspace_failure.working_directory = Some("/workspace".to_owned());
        gate.record(&workspace_failure);

        let mut narrow_failure = outcome(2, ToolKind::Verification, 1);
        narrow_failure.command = "cargo test -p crate_a focused_test".to_owned();
        narrow_failure.working_directory = Some("/workspace".to_owned());
        gate.record(&narrow_failure);

        let mut narrow_success = narrow_failure.clone();
        narrow_success.sequence = 3;
        narrow_success.return_code = Some(0);
        gate.record(&narrow_success);
        assert!(!gate.evidence().completed);

        let mut workspace_success = workspace_failure;
        workspace_success.sequence = 4;
        workspace_success.return_code = Some(0);
        gate.record(&workspace_success);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn verification_scope_keeps_working_directory_package_and_configuration() {
        let mut gate = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test -p crate_a --features sqlite".to_owned();
        failed.working_directory = Some("/workspace/crate_a".to_owned());
        gate.record(&failed);

        let mut other_package = outcome(2, ToolKind::Verification, 0);
        other_package.command = "cargo test -p crate_b --features sqlite".to_owned();
        other_package.working_directory = Some("/workspace/crate_b".to_owned());
        gate.record(&other_package);
        assert!(!gate.evidence().completed);

        let mut other_configuration = outcome(3, ToolKind::Verification, 0);
        other_configuration.command = "cargo test -p crate_a --features postgres".to_owned();
        other_configuration.working_directory = Some("/workspace/crate_a".to_owned());
        gate.record(&other_configuration);
        assert!(!gate.evidence().completed);

        let mut repaired = failed;
        repaired.sequence = 4;
        repaired.return_code = Some(0);
        gate.record(&repaired);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn broader_full_suite_resolves_a_focused_failed_check() {
        let mut gate = CompletionGate::new(true);
        let mut focused = outcome(1, ToolKind::Verification, 1);
        focused.command = "cargo test worker::tests::original_behavior".to_owned();
        focused.working_directory = Some("/workspace".to_owned());
        gate.record(&focused);

        let mut full = outcome(2, ToolKind::Verification, 0);
        full.command = "cargo test".to_owned();
        full.working_directory = Some("/workspace".to_owned());
        gate.record(&full);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn unrelated_green_check_does_not_resolve_failed_runtime_probe() {
        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::Mutation, 0));

        let mut failed_probe = outcome(2, ToolKind::RuntimeProbe, 1);
        failed_probe.command = "curl --fail http://127.0.0.1:8080/health".to_owned();
        gate.record(&failed_probe);

        let mut unrelated = outcome(3, ToolKind::Verification, 0);
        unrelated.command = "cargo test parser::tests::unrelated_behavior".to_owned();
        gate.record(&unrelated);
        assert!(!gate.evidence().completed);
        assert!(gate.evidence().failed_verification_fingerprint.is_some());

        let mut repaired_probe = failed_probe;
        repaired_probe.sequence = 4;
        repaired_probe.return_code = Some(0);
        gate.record(&repaired_probe);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn python_test_import_failure_creates_a_replayable_ticket() {
        let mut gate = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "python3 -m pytest tests/test_service.py".to_owned();
        failed.stderr =
            "ImportError while importing test module\nModuleNotFoundError: No module named 'myapp'"
                .to_owned();
        gate.record(&failed);
        assert!(gate.evidence().failed_verification_fingerprint.is_some());

        let mut unrelated = outcome(2, ToolKind::Verification, 0);
        unrelated.command = "cargo test parser::tests::unrelated_behavior".to_owned();
        gate.record(&unrelated);
        assert!(!gate.evidence().completed);

        failed.sequence = 3;
        failed.return_code = Some(0);
        failed.stderr.clear();
        gate.record(&failed);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn verification_scope_distinguishes_workspace_shell_and_vitest_targets() {
        let cases = [
            ("cargo test --workspace", "cargo test"),
            (
                "test \"$(./tool case-a)\" = expected-a",
                "test \"$(./tool case-b)\" = expected-b",
            ),
            (
                "pnpm exec vitest run src/a.test.ts",
                "pnpm exec vitest run src/b.test.ts",
            ),
        ];
        for (failed_command, unrelated_command) in cases {
            let mut gate = CompletionGate::new(true);
            let mut failed = outcome(1, ToolKind::Verification, 1);
            failed.command = failed_command.to_owned();
            failed.working_directory = Some("/workspace".to_owned());
            gate.record(&failed);

            let mut unrelated = outcome(2, ToolKind::Verification, 0);
            unrelated.command = unrelated_command.to_owned();
            unrelated.working_directory = Some("/workspace".to_owned());
            gate.record(&unrelated);
            assert!(
                !gate.evidence().completed,
                "unrelated command unexpectedly covered {failed_command}: {unrelated_command}"
            );

            failed.sequence = 3;
            failed.return_code = Some(0);
            gate.record(&failed);
            assert!(gate.evidence().completed);
        }
    }

    #[test]
    fn verification_scope_preserves_case_and_normalizes_relative_directories() {
        let mut case_sensitive = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test".to_owned();
        failed.working_directory = Some("/workspace/CaseSensitive".to_owned());
        case_sensitive.record(&failed);

        let mut wrong_case = outcome(2, ToolKind::Verification, 0);
        wrong_case.command = "cargo test".to_owned();
        wrong_case.working_directory = Some("/workspace/casesensitive".to_owned());
        case_sensitive.record(&wrong_case);
        assert!(!case_sensitive.evidence().completed);

        let mut equivalent = CompletionGate::new(true);
        let mut relative = outcome(1, ToolKind::Verification, 1);
        relative.command = "cd crate_a && cargo test".to_owned();
        relative.working_directory = Some("/workspace".to_owned());
        equivalent.record(&relative);

        let mut absolute = outcome(2, ToolKind::Verification, 0);
        absolute.command = "cargo test".to_owned();
        absolute.working_directory = Some("/workspace/crate_a".to_owned());
        equivalent.record(&absolute);
        assert!(equivalent.evidence().completed);

        let mut trailing_cd = CompletionGate::new(true);
        let mut chained = outcome(1, ToolKind::Verification, 1);
        chained.command = "cd crate_a && cargo test && cd ../other".to_owned();
        chained.working_directory = Some("/workspace".to_owned());
        trailing_cd.record(&chained);
        let mut direct = outcome(2, ToolKind::Verification, 0);
        direct.command = "cargo test".to_owned();
        direct.working_directory = Some("/workspace/crate_a".to_owned());
        trailing_cd.record(&direct);
        assert!(trailing_cd.evidence().completed);

        let mut js_trailing_cd = CompletionGate::new(true);
        let mut js_chained = outcome(1, ToolKind::Verification, 1);
        js_chained.command = "cd web && pnpm test && cd ../other".to_owned();
        js_chained.working_directory = Some("/workspace".to_owned());
        let normalized_scope =
            verification_scope(&js_chained.command, js_chained.working_directory.as_deref());
        assert_eq!(
            PathBuf::from(&normalized_scope.working_directory),
            normalize_lexical_path(&PathBuf::from("/workspace").join("web")),
            "verification cwd uses the target platform's native separators"
        );
        #[cfg(windows)]
        assert_eq!(
            PathBuf::from(
                verification_scope("cd web && pnpm test", Some(r"C:\workspace\CaseSensitive"),)
                    .working_directory
            ),
            PathBuf::from(r"C:\workspace\CaseSensitive\web"),
            "Windows drive and backslash input must preserve case while resolving cd"
        );
        js_trailing_cd.record(&js_chained);
        let mut js_direct = outcome(2, ToolKind::Verification, 0);
        js_direct.command = "pnpm test".to_owned();
        js_direct.working_directory = Some("/workspace/web".to_owned());
        let failed_scope = VerificationScope::from_outcome(&js_chained);
        let successful_scope = VerificationScope::from_outcome(&js_direct);
        assert!(
            successful_scope.covers(&failed_scope),
            "successful={successful_scope:?} failed={failed_scope:?}"
        );
        js_trailing_cd.record(&js_direct);
        assert!(js_trailing_cd.evidence().completed);
    }

    #[test]
    fn selector_superset_covers_a_failed_subset() {
        let mut gate = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test -p crate_a".to_owned();
        failed.working_directory = Some("/workspace".to_owned());
        gate.record(&failed);

        let mut broader = outcome(2, ToolKind::Verification, 0);
        broader.command = "cargo test -p crate_a -p crate_b".to_owned();
        broader.working_directory = Some("/workspace".to_owned());
        gate.record(&broader);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn precondition_failure_does_not_create_an_unrunnable_verification_ticket() {
        let mut gate = CompletionGate::new(true);
        let mut missing_directory = outcome(1, ToolKind::Verification, 1);
        missing_directory.command = "cd /missing-project && cargo test".to_owned();
        missing_directory.working_directory = Some("/workspace".to_owned());
        missing_directory.stderr =
            "bash: cd: /missing-project: No such file or directory".to_owned();
        gate.record(&missing_directory);
        assert!(!gate.evidence().completed);
        assert!(gate.evidence().failed_verification_fingerprint.is_none());

        let mut actual_check = outcome(2, ToolKind::Verification, 0);
        actual_check.command = "cargo test".to_owned();
        actual_check.working_directory = Some("/workspace".to_owned());
        gate.record(&actual_check);
        assert!(gate.evidence().completed);
    }

    #[test]
    fn github_delivery_commands_preserve_mutation_and_verification_ordering() {
        assert_eq!(
            classify_command(
                "git commit -m 'fix completion' && git push origin fix/completion",
                120_000,
            ),
            ToolKind::Mutation,
        );
        assert_eq!(
            classify_command("gh pr merge 151 --squash --delete-branch", 120_000),
            ToolKind::Mutation,
        );
        assert_eq!(
            classify_command("gh workflow run 'Auto Release' --repo owner/repo", 120_000),
            ToolKind::Mutation,
        );
        assert_eq!(
            classify_command(
                "pr_url=$(gh pr create --repo owner/repo --base main --head fix/completion); printf '%s\\n' \"$pr_url\"",
                120_000,
            ),
            ToolKind::Mutation,
        );
        assert_eq!(
            classify_command(
                "run_url=$(gh workflow run 'Auto Release' --repo owner/repo); printf '%s\\n' \"$run_url\"",
                120_000,
            ),
            ToolKind::Mutation,
        );
        assert_eq!(
            classify_command("gh pr checks 151 --repo owner/repo --watch", 120_000),
            ToolKind::Verification,
        );
        assert_eq!(
            classify_command(
                "gh run watch 29833845752 --repo owner/repo --interval 20 --exit-status",
                120_000,
            ),
            ToolKind::Verification,
        );

        let mut gate = CompletionGate::default();
        let mut delivery = outcome(1, ToolKind::Mutation, 0);
        delivery.command = "gh workflow run 'Auto Release' --repo owner/repo".to_owned();
        gate.record(&delivery);

        // A watch that actually landed on a red run. (It used to be a timeout
        // here, but a timed-out observation no longer opens a ticket — see
        // `a_timed_out_remote_poll_does_not_open_a_failure_ticket`. The subject
        // of THIS test is scope matching, so it needs a genuine failure.)
        let failed_watch = "gh run watch 29833845752 --repo owner/repo --interval 20 --exit-status";
        let mut observed_failure = outcome(2, classify_command(failed_watch, 120_000), 1);
        observed_failure.command = failed_watch.to_owned();
        observed_failure.stderr = "Run Release completed with 'failure'".to_owned();
        gate.record(&observed_failure);
        assert!(gate.evidence().failed_verification_fingerprint.is_some());

        // Poll cadence is operational, not verification scope. A retry of the
        // same run with a shorter interval must close the timeout ticket.
        let completed_watch =
            "gh run watch 29833845752 --repo owner/repo --interval 5 --exit-status";
        let mut completed = outcome(3, classify_command(completed_watch, 120_000), 0);
        completed.command = completed_watch.to_owned();
        completed.stdout = "Run Release has already completed with 'success'".to_owned();
        gate.record(&completed);
        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);
    }

    /// Field evidence, 2026-07: across one 96-turn session the gate fired on 20
    /// turns and exhausted its recovery budget on 12. Inside those turns, 56 of
    /// 197 tool failures were command timeouts — overwhelmingly CI polls against
    /// a release build that outlives the tool's time cap.
    ///
    /// A timed-out poll is a failed OBSERVATION, not a failed verification. The
    /// pipeline may be perfectly green; we simply stopped looking. Recording it
    /// as a failure opens a ticket that every later success must then "cover",
    /// which is how a delivery turn burns three recovery rounds with a green
    /// run sitting right there. Absence of evidence is not evidence of failure.
    #[test]
    fn a_timed_out_remote_poll_does_not_open_a_failure_ticket() {
        let mut gate = CompletionGate::default();
        let mut merge = outcome(1, ToolKind::Mutation, 0);
        merge.command = "gh pr merge 200 --repo owner/repo --squash".to_owned();
        gate.record(&merge);

        let poll = "gh run view 30152394315 --repo owner/repo --json status,conclusion";
        let mut timed_out = outcome(2, classify_command(poll, 120_000), 1);
        timed_out.command = poll.to_owned();
        timed_out.stdout = "Command timed out after 120s".to_owned();
        gate.record(&timed_out);
        assert!(
            gate.evidence().failed_verification_fingerprint.is_none(),
            "an observation that never landed must not be filed as a failure",
        );

        // A later poll that does land settles the turn, with no stale ticket
        // left to "cover".
        let later = "gh run view 30152394315 --repo owner/repo --json conclusion";
        let mut landed = outcome(3, classify_command(later, 120_000), 0);
        landed.command = later.to_owned();
        landed.stdout = "success".to_owned();
        gate.record(&landed);
        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);
    }

    /// The ticket is redundant, not protective: with no successful observation
    /// the positive requirement already blocks completion. Dropping the ticket
    /// removes a sticky record that outlives a green pipeline; it does not let
    /// an unverified delivery through.
    #[test]
    fn a_timeout_alone_still_leaves_the_delivery_unverified() {
        let mut gate = CompletionGate::default();
        let mut merge = outcome(1, ToolKind::Mutation, 0);
        merge.command = "gh pr merge 200 --repo owner/repo --squash".to_owned();
        gate.record(&merge);

        let poll = "gh run view 30152394315 --repo owner/repo --json status,conclusion";
        let mut timed_out = outcome(2, classify_command(poll, 120_000), 1);
        timed_out.command = poll.to_owned();
        timed_out.stdout = "Command timed out after 120s".to_owned();
        gate.record(&timed_out);

        let evidence = gate.evidence();
        assert!(
            !evidence.completed,
            "a merge whose result was never observed must not be certified",
        );
        assert!(
            evidence
                .blockers
                .iter()
                .any(|blocker| blocker.contains("successful verification")),
            "blockers: {:?}",
            evidence.blockers,
        );
    }

    /// The distinction is about observing versus exercising. A test run that
    /// times out DID run the thing under test and hung; that stays a failure.
    #[test]
    fn a_timed_out_local_test_still_opens_a_failure_ticket() {
        let mut gate = CompletionGate::default();
        let mut edit = outcome(1, ToolKind::Mutation, 0);
        edit.command = "apply_patch src/lib.rs".to_owned();
        gate.record(&edit);

        let suite = "pnpm test";
        let mut timed_out = outcome(2, classify_command(suite, 120_000), 1);
        timed_out.command = suite.to_owned();
        timed_out.stdout = "Command timed out after 120s".to_owned();
        gate.record(&timed_out);
        assert!(
            gate.evidence().failed_verification_fingerprint.is_some(),
            "a suite that hung is a real failure, not a missed observation",
        );
    }

    /// End to end on the shape that actually occurs: merge, then poll the
    /// release run and read its conclusion.
    #[test]
    fn asserting_the_release_conclusion_closes_a_delivery_turn() {
        let mut gate = CompletionGate::default();
        let mut merge = outcome(1, ToolKind::Mutation, 0);
        merge.command = "gh pr merge 200 --repo owner/repo --squash".to_owned();
        gate.record(&merge);
        assert!(!gate.evidence().completed, "a merge alone settles nothing");

        let poll = "gh run view 30152394315 --repo owner/repo --json conclusion -q .conclusion | grep -q success";
        let mut verified = outcome(2, classify_command(poll, 120_000), 0);
        verified.command = poll.to_owned();
        gate.record(&verified);

        let evidence = gate.evidence();
        assert!(evidence.completed, "blockers: {:?}", evidence.blockers);
    }

    #[test]
    fn test_failure_text_that_mentions_a_shell_error_still_opens_a_ticket() {
        let mut gate = CompletionGate::new(true);
        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test parser_reports_unbound_variable".to_owned();
        failed.stderr = "test parser_reports_unbound_variable ... FAILED\nassertion failed: message.contains(\"unbound variable\")".to_owned();
        gate.record(&failed);

        assert!(
            gate.evidence().failed_verification_fingerprint.is_some(),
            "test-runner failures must not be mistaken for shell startup errors"
        );
    }

    #[test]
    fn shell_setup_error_does_not_open_an_unresolvable_verification_ticket() {
        let mut gate = CompletionGate::default();

        let mut mutation = outcome(1, ToolKind::Mutation, 0);
        mutation.command = "edit_file src/pages/Workspace/WorkspacePage.tsx".to_owned();
        mutation.working_directory = Some("/workspace".to_owned());
        gate.record(&mutation);

        // Field reproduction: the intended residual assertion never ran because
        // zsh reserves `status` as a read-only variable. Treat this like a
        // missing command/cwd: diagnostic failure, not a failed verification
        // scope whose exact command must be repeated forever.
        let mut shell_setup_failure = outcome(2, ToolKind::Verification, 1);
        shell_setup_failure.command =
            "status=0; grep -n sidebarCollapsed src/App.tsx || status=$?; test \"$status\" -le 1"
                .to_owned();
        shell_setup_failure.working_directory = Some("/workspace".to_owned());
        shell_setup_failure.stderr = "zsh:4: read-only variable: status".to_owned();
        gate.record(&shell_setup_failure);

        let after_failure = gate.evidence();
        assert!(
            after_failure.failed_verification_fingerprint.is_none(),
            "shell setup failed before the verifier ran: {after_failure:?}"
        );
        assert!(!after_failure.completed);

        // A later real project check must now close the post-failure
        // verification requirement without having to reproduce the broken
        // shell script byte-for-byte.
        let mut repaired_check = outcome(3, ToolKind::Verification, 0);
        repaired_check.command = "cargo test shell_setup_error_does_not_open".to_owned();
        repaired_check.working_directory = Some("/workspace".to_owned());
        repaired_check.stdout = "test result: ok. 1 passed; 0 failed".to_owned();
        gate.record(&repaired_check);

        let completed = gate.evidence();
        assert!(completed.completed, "blockers: {:?}", completed.blockers);
    }

    #[test]
    fn recovery_prompt_requires_a_self_contained_answer_to_the_original_request() {
        let gate = CompletionGate::new(true);
        let prompt = build_completion_recovery_prompt(&gate.evidence());
        assert!(prompt.contains("Treat every rejected draft and this instruction as invisible"));
        assert!(prompt.contains("next response must contain a bounded tool call"));
        assert!(prompt.contains("one bounded diagnostic read"));
        assert!(prompt.contains("in the user's language"));
        assert!(prompt.contains("internal mechanisms such as this gate"));
        assert!(prompt.contains("answer the user's original request directly"));
        assert!(!prompt.contains("adds only the new"));
    }

    #[test]
    fn required_tool_choice_fallback_matches_only_provider_capability_rejections() {
        assert!(provider_rejects_required_tool_choice(
            "Thinking mode does not support this tool_choice"
        ));
        assert!(provider_rejects_required_tool_choice(
            "unsupported value for tool_choice: required"
        ));
        assert!(!provider_rejects_required_tool_choice(
            "HTTP 400: invalid model id"
        ));
        assert!(!provider_rejects_required_tool_choice(
            "HTTP 401: invalid API key"
        ));
    }

    #[test]
    fn completion_recovery_resets_only_for_material_evidence_progress() {
        let mut gate = CompletionGate::new(true);
        let initial = gate.evidence();

        let mut failed = outcome(1, ToolKind::Verification, 1);
        failed.command = "cargo test worker::tests::behavior".to_owned();
        gate.record(&failed);
        let after_failure = gate.evidence();
        assert!(!completion_evidence_made_progress(&initial, &after_failure));

        gate.record(&outcome(2, ToolKind::ReadOnly, 0));
        let after_diagnostic = gate.evidence();
        assert!(!completion_evidence_made_progress(
            &after_failure,
            &after_diagnostic
        ));

        let mut unrelated = outcome(3, ToolKind::Verification, 0);
        unrelated.command = "cargo test parser::tests::unrelated".to_owned();
        gate.record(&unrelated);
        let after_unrelated = gate.evidence();
        assert!(
            !completion_evidence_made_progress(&after_diagnostic, &after_unrelated),
            "before={after_diagnostic:?} after={after_unrelated:?}"
        );

        let mut repaired = failed;
        repaired.sequence = 4;
        repaired.return_code = Some(0);
        gate.record(&repaired);
        let after_repair = gate.evidence();
        assert!(completion_evidence_made_progress(
            &after_unrelated,
            &after_repair
        ));

        let before_mutation = CompletionGate::new(true).evidence();
        let mut mutation_gate = CompletionGate::new(true);
        mutation_gate.record(&outcome(1, ToolKind::Mutation, 0));
        assert!(completion_evidence_made_progress(
            &before_mutation,
            &mutation_gate.evidence()
        ));
    }

    #[test]
    fn ready_prompt_requires_user_language_and_bans_internal_terminology() {
        let prompt = build_completion_ready_prompt();
        assert!(prompt.contains("in the user's language"));
        assert!(prompt.contains("answer the user's original request directly"));
        assert!(prompt.contains("self-contained user-facing result"));
        assert!(prompt.contains("never refer to them"));
        assert!(prompt.contains("Never mention internal mechanisms"));
        assert!(prompt.contains("every modified path"));
        assert!(prompt.contains("revert unrelated changes made by this run"));
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
        assert_eq!(
            effective_command_timeout_sec(
                "gh run watch 30319853083 --repo BumStill/CodeFactory --interval 10 --exit-status",
                120,
                1800,
            ),
            1800
        );
        assert_eq!(
            effective_command_timeout_sec(
                "while gh run view 30319853083 --json status; do sleep 10; done",
                120,
                1800,
            ),
            1800
        );
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
    fn failed_background_start_keeps_the_service_lifecycle_gate_active() {
        let mut gate = CompletionGate::default();
        let mut service = outcome(1, ToolKind::BackgroundServiceStart, 1);
        service.command = "nohup ./server > service.log 2>&1 & echo $! > service.pid".to_owned();
        gate.record(&service);
        gate.record(&outcome(2, ToolKind::Verification, 0));

        let evidence = gate.evidence();
        assert!(!evidence.completed);
        assert_eq!(evidence.last_service_start_sequence, Some(1));
        assert!(evidence
            .blockers
            .iter()
            .any(|item| item.contains("bounded functional probe")));
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
            "/bin/bash: line 0: cd: /missing/project: No such file or directory",
            "IPv6 bind failed: [Errno 1] Operation not permitted",
            "Process dead",
            "PROBE FAIL: response body is not ready",
        ] {
            let mut masked_failure = outcome(6, ToolKind::Verification, 0);
            masked_failure.stdout = output.to_owned();
            masked_failure = masked_failure.with_detected_semantic_failure();
            assert!(!masked_failure.succeeded(), "{output}");
        }
    }

    #[test]
    fn expected_absence_checks_do_not_become_semantic_failures() {
        for (command, output) in [
            (
                "test ! -e stale.pid",
                "stale.pid: No such file or directory",
            ),
            ("! kill -0 4242", "Process not running"),
        ] {
            let mut check = outcome(1, ToolKind::Verification, 0);
            check.command = command.to_owned();
            check.stdout = output.to_owned();
            check = check.with_detected_semantic_failure();

            assert!(check.succeeded(), "{command}: {output}");
        }

        let mut missing_input = outcome(2, ToolKind::Verification, 0);
        missing_input.command = "python3 setup.py build".to_owned();
        missing_input.stderr = "setup.py: No such file or directory".to_owned();
        missing_input = missing_input.with_detected_semantic_failure();
        assert!(!missing_input.succeeded());

        let mut mixed = outcome(3, ToolKind::Verification, 0);
        mixed.command = "test ! -e stale.pid; python3 missing.py || true".to_owned();
        mixed.stderr = "python3: missing.py: No such file or directory".to_owned();
        mixed = mixed.with_detected_semantic_failure();
        assert!(!mixed.succeeded());
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
        assert!(prompt.contains("user-facing result"));
        assert!(prompt.contains("respond with only the most recent verification step"));
    }

    #[test]
    fn completion_summary_prompt_forbids_reopening_verification() {
        let prompt = build_completion_summary_prompt();
        assert!(prompt.contains("evidence is already satisfied"));
        assert!(prompt.contains("Do not call tools"));
        assert!(prompt.contains("rerun checks"));
        assert!(prompt.contains("final user-facing"));
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
        assert!(prompt.contains("separate machine-checked verification"));
        assert!(prompt.contains("batch related reads and edits"));
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
    fn contract_requires_exact_dependencies_observable_controls_and_early_artifacts() {
        let normalized_contract = EXECUTION_CONTRACT
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        assert!(normalized_contract.contains("named tool, library, model, version, or revision"));
        assert!(normalized_contract.contains("before-and-after observable state"));
        assert!(normalized_contract.contains("required output artifact"));
        assert!(normalized_contract.contains("machine-checked assertion"));
        assert!(normalized_contract
            .contains("printing expected and actual values is diagnostic evidence"));
        assert!(normalized_contract.contains("examples copied from the request are smoke checks"));

        let ready = build_completion_ready_prompt();
        assert!(ready.contains("named tool, library, model, version, or revision"));
        assert!(ready.contains("before-and-after observable state"));
        assert!(ready.contains("Request examples are smoke checks only"));

        let convergence = build_budget_convergence_prompt(8, &CompletionEvidence::default());
        assert!(convergence.contains("required output artifact"));
        assert!(convergence.contains("before the final third"));
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
    fn convergence_window_requires_machine_check_after_latest_successful_mutation() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 10,
            last_mutation_sequence: Some(10),
            machine_checked_behavior_required: true,
            last_machine_checked_verification_sequence: Some(8),
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command(
            16,
            &evidence,
            "python3 generate_candidate.py > result.txt",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(!evaluate_budget_command_with_time(
            60,
            Some((600, 900)),
            &evidence,
            "cat another_idea.txt",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            16,
            &evidence,
            "actual=$(./tool 6); test \"$actual\" = 42",
            &ToolKind::Verification,
        )
        .is_allowed());
        assert!(evaluate_budget_command(
            17,
            &evidence,
            "python3 generate_candidate.py > result.txt",
            &ToolKind::Mutation,
        )
        .is_allowed());
    }

    #[test]
    fn convergence_window_blocks_reads_after_example_only_smoke_check() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 11,
            last_mutation_sequence: Some(10),
            machine_checked_behavior_required: true,
            last_machine_checked_verification_sequence: Some(11),
            verification_diversity_required: true,
            last_example_only_verification_sequence: Some(11),
            ..CompletionEvidence::default()
        };

        let denied = evaluate_budget_command_with_time(
            12,
            Some((400, 900)),
            &evidence,
            "cat another_idea.txt",
            &ToolKind::ReadOnly,
        );
        assert!(!denied.is_allowed());
        assert!(matches!(
            denied,
            PolicyDecision::Deny { ref rule, .. } if rule == "verification_diversity"
        ));
        assert!(evaluate_budget_command_with_time(
            12,
            Some((400, 900)),
            &evidence,
            "test \"$(./tool 7)\" = 49",
            &ToolKind::Verification,
        )
        .is_allowed());
    }

    #[test]
    fn final_third_allows_one_failure_diagnostic_then_requires_repair() {
        let mut gate = CompletionGate::new(true);
        gate.record(&outcome(1, ToolKind::Mutation, 0));
        gate.record(&outcome(2, ToolKind::Verification, 1));

        assert!(evaluate_budget_command_with_time(
            20,
            Some((300, 900)),
            &gate.evidence(),
            "sed -n '1,120p' src/worker.rs",
            &ToolKind::ReadOnly,
        )
        .is_allowed());

        gate.record(&outcome(3, ToolKind::ReadOnly, 0));
        let evidence = gate.evidence();
        let denied = evaluate_budget_command_with_time(
            20,
            Some((300, 900)),
            &evidence,
            "cat src/another_module.rs",
            &ToolKind::ReadOnly,
        );
        assert!(matches!(
            denied,
            PolicyDecision::Deny { ref rule, .. } if rule == "failure_repair_loop"
        ));
        assert!(evaluate_budget_command_with_time(
            20,
            Some((300, 900)),
            &evidence,
            "apply_patch <<'PATCH'\n*** Begin Patch\n*** Update File: src/worker.rs\n@@\n-old\n+new\n*** End Patch\nPATCH",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            20,
            Some((300, 900)),
            &evidence,
            "cargo test worker::tests::repairs_failure",
            &ToolKind::Verification,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            20,
            Some((301, 900)),
            &evidence,
            "cat src/another_module.rs",
            &ToolKind::ReadOnly,
        )
        .is_allowed());

        gate.record(&outcome(4, ToolKind::Mutation, 1));
        assert!(evaluate_budget_command_with_time(
            20,
            Some((300, 900)),
            &gate.evidence(),
            "cat src/worker.rs",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
    }

    #[test]
    fn r48_convergence_allows_a_test_harness_mutation_after_example_smoke() {
        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 11,
            last_mutation_sequence: Some(10),
            machine_checked_behavior_required: true,
            last_machine_checked_verification_sequence: Some(11),
            verification_diversity_required: true,
            last_example_only_verification_sequence: Some(11),
            ..CompletionEvidence::default()
        };

        assert!(evaluate_budget_command_with_time(
            12,
            Some((400, 900)),
            &evidence,
            "printf 'test_case()' > tests/test_behavior.py",
            &ToolKind::Mutation,
        )
        .is_allowed());
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
    fn explicit_source_repair_cannot_loop_on_install_without_an_edit() {
        let gate = CompletionGate::new_for_instruction(
            true,
            "Modify this incompatible package, compile the extensions, and install it from source.",
        );
        assert!(gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("requires a source edit")));

        let evidence = CompletionEvidence {
            require_action: true,
            outcome_count: 12,
            last_failure_sequence: Some(12),
            source_delivery_required: true,
            blockers: vec![
                "the explicit source repair requires a source edit before delivery".to_owned(),
            ],
            ..CompletionEvidence::default()
        };

        assert!(!evaluate_budget_command_with_time(
            60,
            Some((590, 900)),
            &evidence,
            "python3 -m pip install -e .",
            &ToolKind::Mutation,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            60,
            Some((590, 900)),
            &evidence,
            "cat build-error.log",
            &ToolKind::ReadOnly,
        )
        .is_allowed());
        assert!(evaluate_budget_command_with_time(
            60,
            Some((590, 900)),
            &evidence,
            "sed -i 's/old_api/new_api/g' src/extension.pyx",
            &ToolKind::Mutation,
        )
        .is_allowed());

        let chinese_gate = CompletionGate::new_for_instruction(
            true,
            "修复这个不兼容的软件包，编译扩展并从源码安装。",
        );
        assert!(chinese_gate
            .evidence()
            .blockers
            .iter()
            .any(|blocker| blocker.contains("requires a source edit")));
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
    fn mutation_starts_a_new_bounded_read_only_inspection_window() {
        let mut tracker = ProgressTracker::new(2);
        assert!(tracker.record(&outcome(1, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker.record(&outcome(2, ToolKind::Mutation, 0)).is_none());
        assert!(tracker.record(&outcome(3, ToolKind::ReadOnly, 0)).is_none());
        let prompt = tracker.record(&outcome(4, ToolKind::ReadOnly, 0)).unwrap();
        assert!(prompt.contains("post-change"));
    }

    #[test]
    fn functional_probe_resets_post_mutation_inspection_pressure() {
        let mut tracker = ProgressTracker::new(2);
        assert!(tracker.record(&outcome(1, ToolKind::Mutation, 0)).is_none());
        assert!(tracker.record(&outcome(2, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker
            .record(&outcome(3, ToolKind::FunctionalProbe { bounded: true }, 0,))
            .is_none());
        assert!(tracker.record(&outcome(4, ToolKind::ReadOnly, 0)).is_none());
        assert!(tracker.record(&outcome(5, ToolKind::ReadOnly, 0)).is_some());
    }

    #[test]
    fn failed_read_only_outcome_does_not_reset_inspection_pressure() {
        let mut tracker = ProgressTracker::new(2);
        assert!(tracker.record(&outcome(1, ToolKind::Mutation, 0)).is_none());
        assert!(tracker.record(&outcome(2, ToolKind::ReadOnly, 0)).is_none());
        let prompt = tracker.record(&outcome(3, ToolKind::ReadOnly, 1)).unwrap();
        assert!(prompt.contains("post-change"));
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
    fn failed_compound_source_edit_requires_a_separate_successful_edit() {
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
        assert_eq!(evidence.last_source_mutation_sequence, None);
        assert!(evidence
            .blockers
            .iter()
            .any(|blocker| blocker.contains("requires a source edit")));
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
    fn failed_or_metadata_only_commands_do_not_satisfy_source_change() {
        for (command, return_code) in [
            ("sed -i 's/old/new/' pkg/main.py", 1),
            ("touch pkg/main.py", 0),
            ("mkdir pkg/generated.py", 0),
            ("rm pkg/main.py", 0),
            ("cp pkg/old.py pkg/main.py", 0),
        ] {
            let mut gate = CompletionGate::new_for_instruction(
                true,
                "Modify this package and install it from source.",
            );
            let mut edit = outcome(1, ToolKind::Mutation, return_code);
            edit.command = command.to_owned();
            gate.record(&edit);

            assert_eq!(
                gate.evidence().last_source_mutation_sequence,
                None,
                "{command}"
            );
        }
    }

    #[test]
    fn common_source_change_wording_enables_the_edit_gate() {
        for instruction in [
            "Update this package and install it from source.",
            "Patch this package and install it from source.",
            "Change this package and install it from source.",
            "更新这个源码包并从源码安装。",
        ] {
            let gate = CompletionGate::new_for_instruction(true, instruction);
            assert!(
                gate.evidence()
                    .blockers
                    .iter()
                    .any(|blocker| blocker.contains("requires a source edit")),
                "{instruction}"
            );
        }
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

        let mut edit = outcome(2, ToolKind::Mutation, 0);
        edit.command = "sed -i 's/old_api/new_api/' pkg/main.py".to_owned();
        gate.record(&edit);

        let mut mixed = outcome(3, ToolKind::ReadOnly, 1);
        mixed.command = "OUT=$(rg 'old_api' --glob '*.py' --glob '*.pyx' .); echo 'SCAN CLEAN: 0 unresolved'; exit 1".to_owned();
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
