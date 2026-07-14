// SPDX-License-Identifier: Apache-2.0
//! Verification engine for Phase 3.
//!
//! After a subagent attempt completes we run a [`VerificationPlan`] against
//! the working directory. Each check emits a [`VerificationResult`] that is
//! persisted to the DB and surfaced in the dashboard.
//!
//! The plan is either supplied explicitly or auto-detected from the project
//! layout by [`detect_verification_plan`].

use crate::util::command_env;
use crate::util::no_window::NoWindow;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Instant;
use tauri::{AppHandle, Emitter};
use tokio::time::{timeout, Duration};

/// Timeout for every command-based check.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationPlan {
    pub checks: Vec<VerificationCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerificationCheck {
    CargoCheck {
        cwd: String,
    },
    CargoTest {
        cwd: String,
        test_filter: Option<String>,
    },
    TscCheck {
        cwd: String,
    },
    PnpmLint {
        cwd: String,
    },
    CustomCommand {
        cwd: String,
        command: String,
        expected_exit_code: i32,
    },
    FileExists {
        path: String,
    },
    FileContains {
        path: String,
        pattern: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub check: String,
    pub passed: bool,
    pub output: String,
    pub duration_ms: u64,
}

// ── Plan detection ────────────────────────────────────────────────────────────

/// Auto-detect a [`VerificationPlan`] from the project root.
pub fn detect_verification_plan(cwd: &str) -> VerificationPlan {
    let mut checks: Vec<VerificationCheck> = Vec::new();

    // Rust project inside src-tauri/
    let tauri_cargo = format!("{}/src-tauri/Cargo.toml", cwd);
    if Path::new(&tauri_cargo).exists() {
        checks.push(VerificationCheck::CargoCheck {
            cwd: format!("{}/src-tauri", cwd),
        });
    } else {
        // Pure Rust project at root
        let root_cargo = format!("{}/Cargo.toml", cwd);
        if Path::new(&root_cargo).exists() {
            checks.push(VerificationCheck::CargoCheck {
                cwd: cwd.to_string(),
            });
        }
    }

    // JS/TS project
    let pkg_json = format!("{}/package.json", cwd);
    if Path::new(&pkg_json).exists() {
        checks.push(VerificationCheck::TscCheck {
            cwd: cwd.to_string(),
        });
        checks.push(VerificationCheck::PnpmLint {
            cwd: cwd.to_string(),
        });
    }

    // Soft check — always present
    checks.push(VerificationCheck::FileExists {
        path: format!("{}/CODEFACTORY.md", cwd),
    });

    VerificationPlan { checks }
}

// ── Runner ────────────────────────────────────────────────────────────────────

/// Run all checks in `plan` and return results.
///
/// Emits `task_verification:{session_id}` after each check with
/// `{ task_id, results: Vec<VerificationResult> }`.
pub async fn run_verification(
    plan: &VerificationPlan,
    app_handle: &AppHandle,
    session_id: &str,
    task_id: &str,
) -> Vec<VerificationResult> {
    let mut results: Vec<VerificationResult> = Vec::new();

    for check in &plan.checks {
        let result = run_single_check(check).await;
        results.push(result);

        // Emit partial progress so the dashboard can update incrementally.
        emit_verification(app_handle, session_id, task_id, &results);
    }

    results
}

// ── Internal helpers ──────────────────────────────────────────────────────────

async fn run_single_check(check: &VerificationCheck) -> VerificationResult {
    match check {
        VerificationCheck::CargoCheck { cwd } => {
            run_command_check("cargo check", cwd, &["cargo", "check"], None, 0).await
        }
        VerificationCheck::CargoTest { cwd, test_filter } => {
            let filter_owned: String = test_filter.clone().unwrap_or_default();
            let name = if filter_owned.is_empty() {
                "cargo test".to_string()
            } else {
                format!("cargo test {}", filter_owned)
            };
            let mut args = vec!["cargo", "test"];
            if !filter_owned.is_empty() {
                args.push(&filter_owned);
            }
            run_command_check(&name, cwd, &args, None, 0).await
        }
        VerificationCheck::TscCheck { cwd } => {
            run_command_check("tsc --noEmit", cwd, &["npx", "tsc", "--noEmit"], None, 0).await
        }
        VerificationCheck::PnpmLint { cwd } => {
            // Check if lint script exists in package.json first.
            let pkg_path = format!("{}/package.json", cwd);
            match std::fs::read_to_string(&pkg_path) {
                Err(_) => VerificationResult {
                    check: "pnpm lint".into(),
                    passed: true,
                    output: "package.json not found, skipping lint".into(),
                    duration_ms: 0,
                },
                Ok(raw) => {
                    let has_lint = serde_json::from_str::<serde_json::Value>(&raw)
                        .ok()
                        .and_then(|v| v.get("scripts")?.get("lint").cloned())
                        .is_some();
                    if has_lint {
                        run_command_check("pnpm lint", cwd, &["pnpm", "lint"], None, 0).await
                    } else {
                        VerificationResult {
                            check: "pnpm lint".into(),
                            passed: true,
                            output: "No lint script in package.json, skipping".into(),
                            duration_ms: 0,
                        }
                    }
                }
            }
        }
        VerificationCheck::CustomCommand {
            cwd,
            command,
            expected_exit_code,
        } => run_command_check(command, cwd, &[], Some(command), *expected_exit_code).await,
        VerificationCheck::FileExists { path } => {
            let t = Instant::now();
            let exists = Path::new(path).exists();
            VerificationResult {
                check: format!("file exists: {}", path),
                passed: exists,
                output: if exists {
                    format!("{} exists", path)
                } else {
                    format!("{} not found", path)
                },
                duration_ms: t.elapsed().as_millis() as u64,
            }
        }
        VerificationCheck::FileContains { path, pattern } => {
            let t = Instant::now();
            let result = check_file_contains(path, pattern);
            VerificationResult {
                check: format!("file contains '{}'", pattern),
                passed: result.passed,
                output: result.output,
                duration_ms: t.elapsed().as_millis() as u64,
            }
        }
    }
}

struct CheckResult {
    passed: bool,
    output: String,
}

fn check_file_contains(path: &str, pattern: &str) -> CheckResult {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return CheckResult {
                passed: false,
                output: format!("Could not read {}: {}", path, e),
            };
        }
    };
    match regex::Regex::new(pattern) {
        Ok(re) => {
            if re.is_match(&content) {
                CheckResult {
                    passed: true,
                    output: format!("Pattern '{}' found in {}", pattern, path),
                }
            } else {
                CheckResult {
                    passed: false,
                    output: format!("Pattern '{}' NOT found in {}", pattern, path),
                }
            }
        }
        Err(e) => CheckResult {
            passed: false,
            output: format!("Invalid regex '{}': {}", pattern, e),
        },
    }
}

/// Run a shell command with a 60-second timeout and capture combined output.
///
/// `shell_args` takes precedence over `command_str` when non-empty.
/// When `shell_args` is empty and `command_str` is Some, we run via `cmd /C`
/// on Windows so the user can supply arbitrary shell expressions.
async fn run_command_check(
    check_name: &str,
    cwd: &str,
    shell_args: &[&str],
    command_str: Option<&str>,
    expected_exit_code: i32,
) -> VerificationResult {
    let t = Instant::now();

    let task_result = if !shell_args.is_empty() {
        // Direct invocation: first element is the binary.
        let (prog, args) = shell_args.split_first().expect("shell_args non-empty");
        let mut cmd = tokio::process::Command::new(prog).no_window();
        cmd.args(args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command_env::apply_developer_path(&mut cmd);

        timeout(CHECK_TIMEOUT, async move {
            match cmd.output().await {
                Ok(out) => {
                    let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    if !stderr.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&stderr);
                    }
                    let code = out.status.code().unwrap_or(-1);
                    (code == expected_exit_code, combined)
                }
                Err(e) => (false, format!("Failed to spawn: {}", e)),
            }
        })
        .await
    } else if let Some(cmd_str) = command_str {
        let cmd_owned = cmd_str.to_string();
        let cwd_owned = cwd.to_string();
        timeout(CHECK_TIMEOUT, async move {
            let shell = command_env::shell_invocation(&cmd_owned);
            let mut cmd = tokio::process::Command::new(shell.program).no_window();
            cmd.args(shell.args)
                .current_dir(&cwd_owned)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command_env::apply_developer_path(&mut cmd);
            match cmd.output().await {
                Ok(out) => {
                    let mut combined = String::from_utf8_lossy(&out.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    if !stderr.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&stderr);
                    }
                    let code = out.status.code().unwrap_or(-1);
                    (code == expected_exit_code, combined)
                }
                Err(e) => (false, format!("Failed to spawn: {}", e)),
            }
        })
        .await
    } else {
        Ok((false, "No command provided".into()))
    };

    let duration_ms = t.elapsed().as_millis() as u64;

    match task_result {
        Ok((passed, output)) => VerificationResult {
            check: check_name.to_string(),
            passed,
            output,
            duration_ms,
        },
        Err(_elapsed) => VerificationResult {
            check: check_name.to_string(),
            passed: false,
            output: format!("Check timed out after {}s", CHECK_TIMEOUT.as_secs()),
            duration_ms,
        },
    }
}

#[derive(Serialize, Clone)]
struct VerificationEventPayload<'a> {
    task_id: &'a str,
    results: &'a [VerificationResult],
}

fn emit_verification(
    app: &AppHandle,
    session_id: &str,
    task_id: &str,
    results: &[VerificationResult],
) {
    let event = format!("task_verification:{}", session_id);
    if let Err(e) = app.emit(&event, VerificationEventPayload { task_id, results }) {
        tracing::warn!("failed to emit verification event: {}", e);
    }
}
