// SPDX-License-Identifier: Apache-2.0
//! M224 共享进程沙箱（阶段 1，macOS Seatbelt）。
//!
//! 目标（见 docs/specs/feature-specs/m224-os-sandbox-trusted-commands.md）：
//! - R2：用 Seatbelt 启动命令，全盘可读、写入默认全部拒绝，只放行登记白名单；子进程继承限制。
//! - R3（编排方裁决）：不写针对应用数据目录的整体 deny，改为"默认拒绝 + 白名单"，
//!   白名单只包含经身份验证、已登记的那一个工作区（以及调用方显式登记的支撑路径）。
//!   应用数据目录根、settings.json、数据库、日志、其他会话工作区都不在白名单里，
//!   因此必须不可写。
//! - R7：沙箱运行时不可用（`sandbox-exec` 缺失 / 策略生成失败）时返回错误，调用方退回审批
//!   路径，绝不静默无沙箱执行。
//! - R8：被拒绝时给出可读诊断（沙箱拦截 + 具体路径 + 该怎么办），不是裸 `Operation not permitted`。
//! - R6：门禁绕过命令（`--no-verify`、`CODEFACTORY_SKIP_SYNC_GATE`、改 `core.hooksPath`）
//!   直接拒绝并给修复指引，不进审批。
//!
//! 阶段 1 的已知边界：只要 `NetworkPolicy::Enabled` 就允许联网（R4）；本模块尚未接入 bash 工具
//! 的前台/后台启动点，接入工作单独进行（见 Implementation Notes）。

use std::path::{Path, PathBuf};

/// Seatbelt 运行时在 macOS 上的固定路径。
pub const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// 阶段 1：网络策略（R4 只记录边界，不声称已隔离）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// 允许联网（git fetch / cargo / pnpm 需要）。
    Enabled,
    /// 拒绝联网。
    Disabled,
}

/// 一条写入白名单条目：整棵子树或单个文件（literal）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WritablePath {
    path: PathBuf,
    recursive: bool,
}

impl WritablePath {
    /// 允许写入整棵子树（如会话工作区）。
    pub fn tree(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            recursive: true,
        }
    }

    /// 只允许写入这一个文件/目录项（如 git 元数据的个别文件）。
    pub fn literal(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            recursive: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_recursive(&self) -> bool {
        self.recursive
    }
}

/// 实际要启动的进程描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLaunch {
    pub program: String,
    pub args: Vec<String>,
}

/// 一个已解析、已规范化的沙箱实例。
#[derive(Clone, Debug)]
pub struct CommandSandbox {
    workspace: PathBuf,
    writable: Vec<WritablePath>,
    network: NetworkPolicy,
    runtime: PathBuf,
    profile: String,
}

impl CommandSandbox {
    /// 构建沙箱。任何前提不满足都返回 `Err`，由调用方 fail-closed 退回审批路径（R7）。
    pub fn new(
        workspace: &Path,
        writable: Vec<WritablePath>,
        network: NetworkPolicy,
    ) -> Result<Self, String> {
        if !workspace.is_absolute() {
            return Err(format!(
                "M224：工作区必须是绝对路径，收到 {}（系统自动重试也不会绕过该检查）",
                workspace.display()
            ));
        }
        let canonical_workspace = canonical(workspace)?;
        let runtime = probe_program(MACOS_SANDBOX_EXEC)?;
        let mut canonical_writable = Vec::with_capacity(writable.len());
        for entry in writable {
            canonical_writable.push(WritablePath {
                path: canonical(&entry.path)?,
                recursive: entry.recursive,
            });
        }
        let profile = build_profile(&canonical_workspace, &canonical_writable, network);
        Ok(Self {
            workspace: canonical_workspace,
            writable: canonical_writable,
            network,
            runtime,
            profile,
        })
    }

    /// 生成的 Seatbelt 策略文本（同时用于诊断与测试断言）。
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// 已规范化的工作区路径。
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// 沙箱运行时路径。
    pub fn runtime(&self) -> &Path {
        &self.runtime
    }

    /// 已规范化的显式写入白名单（不含工作区本身；工作区恒在白名单首项）。
    pub fn writable(&self) -> &[WritablePath] {
        &self.writable
    }

    pub fn network_policy(&self) -> NetworkPolicy {
        self.network
    }

    /// 把一个命令包装成受沙箱约束的启动调用。
    pub fn wrap(&self, program: &str, args: &[String]) -> CommandLaunch {
        let mut argv = Vec::with_capacity(args.len() + 3);
        argv.push("-p".to_string());
        argv.push(self.profile.clone());
        argv.push(program.to_string());
        argv.extend(args.iter().cloned());
        CommandLaunch {
            program: self.runtime.display().to_string(),
            args: argv,
        }
    }

    /// R8：把沙箱拒绝翻译成模型能照着改的诊断；绝不建议自动提权重试。
    pub fn failure_hint(&self, stderr: &str) -> String {
        let workspace = self.workspace.display();
        match denied_path(stderr) {
            Some(path) => format!(
                "沙箱拦截：{path} 不在本会话登记工作区 {workspace} 内，写入被系统拒绝。\
                 本会话只有已登记工作区可写；请把文件写入改到工作区内，或改用受控文件工具处理应用数据。\
                 沙箱不会自动提权重试（系统自动重试同样无法越过边界）。"
            ),
            None => format!(
                "沙箱拦截：命令被沙箱拒绝（未能从输出解析出具体路径）。\
                 本会话只有已登记工作区 {workspace} 可写；请检查命令写入的目标路径是否在工作区内。"
            ),
        }
    }
}

/// 探测沙箱运行时是否可用（R7：启动时一次，状态可查）。
pub fn probe_program(path: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(path);
    let metadata = std::fs::metadata(&candidate).map_err(|error| {
        format!("M224：沙箱运行时不可用（{path}）：{error}；退回审批路径，不执行无沙箱命令")
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "M224：沙箱运行时不可用（{path} 不是可执行文件）；退回审批路径，不执行无沙箱命令"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "M224：沙箱运行时不可执行（{path} 缺少执行位）；退回审批路径，不执行无沙箱命令"
            ));
        }
    }
    Ok(candidate)
}

/// R6：门禁绕过判定，直接拒绝并返回修复指引（不进审批）。
pub fn gate_bypass(command: &str) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    let prefix = "禁止绕过仓库同步与提交校验门禁（main 保护）";

    if lower.contains("codefactory_skip_sync_gate") {
        return Some(format!(
            "{prefix}：检测到 CODEFACTORY_SKIP_SYNC_GATE，该开关只能由用户在处理紧急热修时显式使用。\
             修复指引：先 git fetch origin main 并合并最新 main，再正常提交。"
        ));
    }

    if lower.contains("--no-verify") && lower.contains("git") {
        return Some(format!(
            "{prefix}：检测到 --no-verify，提交必须经过 pre-commit 钩子。\
             修复指引：先 git fetch --prune origin main && git merge origin/main，再正常 git commit。"
        ));
    }

    if let Some(rest) = lower.split("core.hookspath").nth(1) {
        let rest = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
        let value = rest
            .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .next()
            .unwrap_or("")
            .trim_end_matches([';', '&', '|']);
        // 只有把钩子目录显式设成 .githooks 才算合规；取消（--unset）或指向别处都算绕过。
        let allowed = value == ".githooks" || value == "./.githooks";
        if !allowed {
            let shown = if value.is_empty() {
                "（取消设置）".to_string()
            } else {
                value.to_string()
            };
            return Some(format!(
                "{prefix}：检测到修改 core.hooksPath {shown}，钩子目录必须保持 .githooks。\
                 修复指引：运行 git config core.hooksPath .githooks 恢复，再重试原操作。"
            ));
        }
    }

    None
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| {
        format!(
            "M224：路径无法规范化（{}）：{error}；策略生成失败即 fail-closed",
            path.display()
        )
    })
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out
}

fn build_profile(workspace: &Path, writable: &[WritablePath], network: NetworkPolicy) -> String {
    let mut profile = String::new();
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n");
    profile.push_str("(allow process*)\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow mach-lookup)\n");
    profile.push_str("(allow file-read*)\n");
    profile.push_str("(deny file-write*)\n");
    // R3：白名单先写"登记工作区"，再写调用方显式登记的支撑路径；写在 deny 之后才生效。
    let mut entries = vec![WritablePath::tree(workspace.to_path_buf())];
    entries.extend(writable.iter().cloned());
    for entry in &entries {
        let literal = escape(&entry.path.to_string_lossy());
        profile.push_str(&format!("(allow file-write* (literal \"{literal}\"))\n"));
        if entry.recursive {
            profile.push_str(&format!("(allow file-write* (subpath \"{literal}\"))\n"));
        }
    }
    profile.push_str("(allow file-write* (literal \"/dev/null\"))\n");
    if network == NetworkPolicy::Enabled {
        profile.push_str("(allow network*)\n");
    }
    profile
}

/// 从 stderr 里挑出被拒绝的路径（Seatbelt / 语言的错误信息格式不一，按 token 兜底）。
fn denied_path(stderr: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for token in stderr.split(|c: char| {
        c.is_whitespace() || matches!(c, '\'' | '"' | '(' | ')' | '[' | ']' | ',' | ';')
    }) {
        let cleaned = token.trim_end_matches([':', '.', '。', '，']);
        if cleaned.starts_with('/') && cleaned.len() > 1 {
            last = Some(cleaned.to_string());
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_bypass_flags_protected_operations() {
        assert!(gate_bypass("git commit --no-verify -m x").is_some());
        assert!(gate_bypass("git config --unset core.hooksPath").is_some());
        assert!(gate_bypass("git config core.hooksPath .githooks").is_none());
        assert!(gate_bypass("cargo test").is_none());
    }

    #[test]
    fn profile_keeps_writes_denied_by_default() {
        let profile = build_profile(Path::new("/private/var/ws"), &[], NetworkPolicy::Enabled);
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(allow file-write* (subpath \"/private/var/ws\"))"));
        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn denied_path_extracts_last_absolute_token() {
        let stderr =
            "PermissionError: [Errno 1] Operation not permitted: '/private/var/settings.json'";
        assert_eq!(
            denied_path(stderr).as_deref(),
            Some("/private/var/settings.json")
        );
    }
}
