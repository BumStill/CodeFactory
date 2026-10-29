//! M193：用「卡住的证据」代替固定工具次数维持收敛。
//!
//! 旧实现按「本回合第 N 次工具调用」无条件注入提示，正常的长任务在第 N 次
//! 调用之后每一轮都会被打断。这里的检测器只看可观察的循环特征（沿用
//! OpenHands `StuckDetector` 的场景划分与本仓库已有 completion gate 证据语义）：
//!
//! 1. `RepeatedActionObservation`：连续 4 次相同调用、相同结果；
//! 2. `RepeatedActionError`：连续 3 次相同调用且都失败；
//! 3. `AlternatingLoop`：A/B 交替（调用与结果都交替）；
//! 4. `NoProgress`：连续若干次调用都没有产生新证据。
//!
//! 「新证据」= 出现一个此前没见过的工具结果（结果指纹去重）。写入成功等
//! 变化会自然换出新指纹；反复读同一文件不会。
//!
//! 检测器本身不产出对话消息，也不写库：调用方把提示挂到工具结果通道上。

use std::collections::HashSet;

/// 没有新证据时允许继续的工具调用次数（`NoProgress` 的默认阈值）。
pub const DEFAULT_NO_PROGRESS_LIMIT: usize = 12;

/// 触发信号后，至少要再有这么多步才允许下一次触发，避免同一循环里刷屏。
const COOLDOWN_STEPS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StuckSignal {
    RepeatedActionObservation { call: String, repeats: usize },
    RepeatedActionError { call: String, repeats: usize },
    AlternatingLoop { first: String, second: String },
    NoProgress { steps: usize },
}

impl StuckSignal {
    /// 交给模型看的收敛提示：只说明观察到的卡住事实和该怎么做。
    pub fn hint(&self, convergence_scope: &str) -> String {
        match self {
            StuckSignal::RepeatedActionObservation { call, repeats } => format!(
                "【卡住信号】连续 {repeats} 次相同的工具调用返回了相同结果（{call}）。该结果已经拿到，请直接复用，不要再重复调用；{convergence_scope}。"
            ),
            StuckSignal::RepeatedActionError { call, repeats } => format!(
                "【卡住信号】连续 {repeats} 次相同的工具调用都失败（{call}）。请换一条路径或说明外部阻断，不要原样重试；{convergence_scope}。"
            ),
            StuckSignal::AlternatingLoop { first, second } => format!(
                "【卡住信号】工具调用在 {first} 与 {second} 之间交替循环且没有新结果。请离开这个循环；{convergence_scope}。"
            ),
            StuckSignal::NoProgress { steps } => format!(
                "【卡住信号】最近 {steps} 次工具调用没有产生新证据。请复用已有证据，停止重复探测；{convergence_scope}。"
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct Step {
    call: String,
    observation: String,
    failed: bool,
}

/// 单轮的卡住检测器。每轮用户输入重置一次（与 OpenHands 交互模式只看最后
/// 一条用户消息之后的记录一致）。
#[derive(Debug)]
pub struct StuckDetector {
    no_progress_limit: usize,
    steps: Vec<Step>,
    seen_observations: HashSet<String>,
    last_progress_at: usize,
    fired_at: Option<usize>,
}

impl StuckDetector {
    pub fn new(no_progress_limit: usize) -> Self {
        Self {
            no_progress_limit: no_progress_limit.max(1),
            steps: Vec::new(),
            seen_observations: HashSet::new(),
            last_progress_at: 0,
            fired_at: None,
        }
    }

    pub fn reset(&mut self) {
        self.steps.clear();
        self.seen_observations.clear();
        self.last_progress_at = 0;
        self.fired_at = None;
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// 记录一次工具调用的结果，返回本次是否观察到卡住信号。
    pub fn observe(&mut self, call: &str, observation: &str, failed: bool) -> Option<StuckSignal> {
        let call = normalize_call(call);
        let observation = observation_fingerprint(observation);
        let failed = failed || looks_failed(&observation);

        let is_new_evidence = !self.seen_observations.contains(&observation);
        if is_new_evidence {
            self.seen_observations.insert(observation.clone());
        }

        self.steps.push(Step {
            call,
            observation,
            failed,
        });
        if is_new_evidence {
            self.last_progress_at = self.steps.len();
        }

        // 触发后冷却，至少再走 COOLDOWN_STEPS 步才重新判断。
        if let Some(fired_at) = self.fired_at {
            if self.steps.len() < fired_at.saturating_add(COOLDOWN_STEPS) {
                return None;
            }
        }

        let signal = self.detect();
        if signal.is_some() {
            self.fired_at = Some(self.steps.len());
        }
        signal
    }

    fn detect(&self) -> Option<StuckSignal> {
        let len = self.steps.len();
        if len < 3 {
            return None;
        }

        // 1) 连续 4 次同调用、同结果。
        if len >= 4 {
            let window = &self.steps[len - 4..];
            let call = &window[0].call;
            if window.iter().all(|step| &step.call == call)
                && window
                    .iter()
                    .all(|step| step.observation == window[0].observation)
            {
                return Some(StuckSignal::RepeatedActionObservation {
                    call: call.clone(),
                    repeats: 4,
                });
            }
        }

        // 2) 连续 3 次同调用且都失败。
        let window = &self.steps[len - 3..];
        let call = &window[0].call;
        if window.iter().all(|step| &step.call == call)
            && window.iter().all(|step| step.failed)
        {
            return Some(StuckSignal::RepeatedActionError {
                call: call.clone(),
                repeats: 3,
            });
        }

        // 3) A/B 交替：调用与结果都交替，且 A != B。
        if len >= 4 {
            let window = &self.steps[len - 4..];
            let pattern = [0usize, 1, 0, 1];
            let first = &window[0];
            let second = &window[1];
            if first.call != second.call
                && first.observation != second.observation
                && window.iter().enumerate().all(|(index, step)| {
                    let reference = if pattern[index] == 0 { first } else { second };
                    step.call == reference.call && step.observation == reference.observation
                })
            {
                return Some(StuckSignal::AlternatingLoop {
                    first: first.call.clone(),
                    second: second.call.clone(),
                });
            }
        }

        // 4) 长时间没有新证据。
        let without_new_evidence = len.saturating_sub(self.last_progress_at);
        if without_new_evidence >= self.no_progress_limit {
            return Some(StuckSignal::NoProgress {
                steps: without_new_evidence,
            });
        }

        None
    }
}

/// 调用指纹：工具名 + 参数（压掉空白，避免格式差异掩盖「同一次调用」）。
fn normalize_call(call: &str) -> String {
    let collapsed = call.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, 160)
}

fn observation_fingerprint(observation: &str) -> String {
    let collapsed = observation.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, 400)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

/// 工具失败判定：既有本仓库的 "Error:" 文本约定，也认结构化失败字段。
pub fn looks_failed(observation: &str) -> bool {
    let trimmed = observation.trim_start();
    let lowered = trimmed.to_ascii_lowercase();
    if lowered.starts_with("error")
        || lowered.starts_with("failed")
        || lowered.starts_with("tool execution failed")
        || lowered.starts_with("blocked")
    {
        return true;
    }
    if trimmed.starts_with("错误") || trimmed.starts_with("执行失败") || trimmed.starts_with("已拒绝")
    {
        return true;
    }
    let compact = lowered.replace(' ', "");
    compact.contains("\"success\":false") || compact.contains("\"ok\":false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_identical_call_and_result_is_stuck() {
        let mut detector = StuckDetector::new(DEFAULT_NO_PROGRESS_LIMIT);
        let mut signal = None;
        for _ in 0..4 {
            signal = detector.observe("read_file {}", "same file body", false);
        }
        assert_eq!(
            signal,
            Some(StuckSignal::RepeatedActionObservation {
                call: "read_file {}".into(),
                repeats: 4
            })
        );
        assert!(signal.unwrap().hint("只执行尚未完成的最小步骤").contains("卡住信号"));
    }

    #[test]
    fn same_call_with_changing_results_is_not_stuck() {
        let mut detector = StuckDetector::new(DEFAULT_NO_PROGRESS_LIMIT);
        for index in 0..6 {
            assert_eq!(
                detector.observe("read_file {}", &format!("body {index}"), false),
                None
            );
        }
    }

    #[test]
    fn long_task_with_fresh_evidence_never_trips() {
        // 40 次工具调用（旧实现固定的阈值）全部有新证据：不得触发任何信号。
        let mut detector = StuckDetector::new(DEFAULT_NO_PROGRESS_LIMIT);
        for index in 0..40 {
            assert_eq!(
                detector.observe(
                    &format!("read_file {{\"path\":\"f{index}.rs\"}}"),
                    &format!("content of f{index}"),
                    false
                ),
                None,
                "step {index} must not be reported as stuck"
            );
        }
    }

    #[test]
    fn repeated_identical_failures_are_stuck() {
        let mut detector = StuckDetector::new(DEFAULT_NO_PROGRESS_LIMIT);
        let mut signal = None;
        for _ in 0..3 {
            signal = detector.observe("bash", "Error: command not found: foo", true);
        }
        assert_eq!(
            signal,
            Some(StuckSignal::RepeatedActionError {
                call: "bash".into(),
                repeats: 3
            })
        );
    }

    #[test]
    fn alternating_loop_is_stuck() {
        let mut detector = StuckDetector::new(DEFAULT_NO_PROGRESS_LIMIT);
        let mut signal = None;
        for index in 0..4 {
            let (call, observation) = if index % 2 == 0 {
                ("grep A", "no matches for A")
            } else {
                ("grep B", "no matches for B")
            };
            signal = detector.observe(call, observation, false);
        }
        assert_eq!(
            signal,
            Some(StuckSignal::AlternatingLoop {
                first: "grep A".into(),
                second: "grep B".into()
            })
        );
    }

    #[test]
    fn no_new_evidence_trips_the_progress_limit() {
        let mut detector = StuckDetector::new(4);
        let mut signals = Vec::new();
        for index in 0..6 {
            // 调用各不相同、但结果指纹完全一致：这是「反复探测却没有新证据」。
            let call = format!("grep {{\"pattern\":\"todo-{index}\"}}");
            if let Some(signal) = detector.observe(&call, "no matches found", false) {
                signals.push(signal);
            }
        }
        assert!(
            signals
                .iter()
                .any(|signal| matches!(signal, StuckSignal::NoProgress { steps } if *steps >= 4)),
            "expected a NoProgress signal, got {signals:?}"
        );
    }

    #[test]
    fn fires_at_most_once_per_cooldown_window() {
        let mut detector = StuckDetector::new(3);
        let mut fired = 0;
        for index in 0..6 {
            let observation = if index == 5 { "fresh evidence" } else { "stale" };
            if detector.observe("grep todo", observation, false).is_some() {
                fired += 1;
            }
        }
        assert!(fired <= 1, "expected at most one signal in the window, got {fired}");
    }

    #[test]
    fn reset_clears_history() {
        let mut detector = StuckDetector::new(2);
        let _ = detector.observe("grep", "same", false);
        let _ = detector.observe("grep", "same", false);
        detector.reset();
        assert!(detector.is_empty());
        assert_eq!(detector.observe("grep", "same", false), None);
    }

    #[test]
    fn detects_structured_and_textual_failures() {
        assert!(looks_failed("Error: permission denied"));
        assert!(looks_failed("执行失败：命令不存在"));
        assert!(looks_failed("{\"success\":false,\"message\":\"nope\"}"));
        assert!(!looks_failed("here is the file body"));
    }
}
