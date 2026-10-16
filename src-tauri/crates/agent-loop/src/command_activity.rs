// SPDX-License-Identifier: Apache-2.0

/// Persisted execution origin. `now_ms` is explicit so heartbeats and restart
/// projections share one clock contract without sleeping in tests.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandActivityTimer {
    pub execution_started_at_ms: u64,
}

impl CommandActivityTimer {
    pub fn new(now_ms: u64) -> Self {
        Self { execution_started_at_ms: now_ms }
    }

    pub fn elapsed(&self, now_ms: u64) -> std::time::Duration {
        std::time::Duration::from_millis(now_ms.saturating_sub(self.execution_started_at_ms))
    }

    pub fn label(&self, now_ms: u64, subject: &str, command: Option<&str>) -> String {
        let seconds = self.elapsed(now_ms).as_secs();
        let elapsed = if seconds < 60 { format!("{} 秒", seconds.max(1)) }
                      else { format!("{} 分钟", seconds / 60) };
        if seconds >= 600 {
            if let Some(command) = command {
                // Never project raw arguments, assignments, URLs, or paths into
                // persisted activity. Recognized build/test commands get a verb.
                let mut words = command.split_whitespace();
                let executable = words.next().unwrap_or("");
                let verb = words.next().unwrap_or("");
                let summary = match executable {
                    "cargo" | "pnpm" | "npm" | "yarn" | "python" | "python3" | "node" | "git" | "curl" | "echo" => {
                        if matches!(verb, "test" | "build" | "check" | "install" | "run" | "fetch" | "status") {
                            format!("{executable} {verb}")
                        } else { executable.to_string() }
                    }
                    _ => subject.to_string(),
                };
                return format!("{summary} 已运行约 {elapsed}");
            }
        }
        format!("{subject}仍在运行（约 {elapsed}）")
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m90_heartbeats_and_restart_keep_execution_origin() {
        let started = 1_000_000;
        let timer = CommandActivityTimer::new(started);
        for minute in 1..=26 {
            let now = started + minute * 60_000;
            let label = timer.label(now, "命令", Some("cargo test --workspace"));
            assert!(label.contains(&format!("{minute} 分钟")), "{label}");
        }
        let stored = serde_json::to_string(&timer).unwrap();
        let restored: CommandActivityTimer = serde_json::from_str(&stored).unwrap();
        assert_eq!(restored.label(started + 26 * 60_000, "命令", Some("cargo test --workspace")),
                   "cargo test 已运行约 26 分钟");
    }

    #[test]
    fn m90_threshold_and_clock_rollback() {
        let timer = CommandActivityTimer::new(1000);
        assert_eq!(timer.label(600_000, "命令", Some("cargo test")), "命令仍在运行（约 9 分钟）");
        assert_eq!(timer.label(601_000, "命令", Some("cargo test")), "cargo test 已运行约 10 分钟");
        assert_eq!(timer.label(0, "命令", Some("cargo test")), "命令仍在运行（约 1 秒）");
        assert_eq!(timer.label(721_000, "工具", None), "工具仍在运行（约 12 分钟）");
    }

    #[test]
    fn m90_command_summary_does_not_disclose_arguments() {
        let timer = CommandActivityTimer::new(0);
        for command in ["curl -H 'Authorization: Bearer secret' https://example.invalid", "TOKEN=secret cargo test", "echo secret", "cargo --token=secret", "猫秘密 secret", "https://secret@example.invalid"] {
            let label = timer.label(1_560_000, "命令", Some(command));
            assert!(!label.contains("secret"), "{label}");
            assert!(!label.contains("秘密"), "{label}");
            assert!(label.chars().count() < 100);
        }
        assert_eq!(timer.label(1_020_000, "命令", Some("pnpm build --filter synthetic")), "pnpm build 已运行约 17 分钟");
    }
}
