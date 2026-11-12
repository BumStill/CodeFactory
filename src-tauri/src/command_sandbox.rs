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
//! 网络策略分三档：`Disabled`、`RegistryProxy`（自动放行命令只连宿主出口代理与回环）、
//! `Full`（用户已批准的命令）。配置文件、mach 服务和启动器程序的收紧始终写在策略里。

use std::path::{Path, PathBuf};

/// Seatbelt 运行时在 macOS 上的固定路径。
pub const MACOS_SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// 网络策略。自动放行命令用 [`RegistryProxy`]；用户批准后的命令用 [`Full`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// 拒绝联网。
    Disabled,
    /// 只允许连到宿主出口代理端口，以及本机回环（测试里的本地服务）。
    RegistryProxy { port: u16 },
    /// 用户已批准的命令：网络不再由代理白名单限制。凭据仍不注入子进程。
    Full,
}

/// 调用方按本次会话填入的额外拒绝项。路径可以尚不存在，策略仍写字面量。
#[derive(Clone, Debug, Default)]
pub struct SandboxConstraints {
    /// `file-read*` 拒绝（目录用 subpath，文件同时写 literal）。
    pub read_deny: Vec<PathBuf>,
    /// dispatch 目录。`require_dispatch_deny` 为真且这里为空，或路径不是绝对路径时，构建失败。
    pub dispatch_dirs: Vec<PathBuf>,
    pub require_dispatch_deny: bool,
    /// 需要拒绝 `network-outbound` 的本机套接字。
    pub socket_paths: Vec<PathBuf>,
    /// 即便在回环上也不许连接的宿主持有端口（例如浏览器桥）。
    pub denied_loopback_ports: Vec<u16>,
}

/// mach-lookup 显式白名单。每一项都是工具链要能工作所必需的系统服务。
/// 不在此列：LaunchServices / AppleEvents、launchd 作业提交、钥匙串 / SecurityServer、剪贴板。
const MACH_LOOKUP_ALLOW: &[(&str, &str)] = &[
    ("com.apple.system.logger", "系统日志"),
    ("com.apple.logd", "统一日志"),
    ("com.apple.syslogd", "syslog"),
    ("com.apple.cfprefsd.agent", "用户偏好，工具链启动会读"),
    ("com.apple.cfprefsd.daemon", "系统偏好"),
    ("com.apple.distributed_notifications@Uv3", "进程内通知"),
    ("com.apple.distributed_notifications@1v3", "进程内通知（另一代）"),
    ("com.apple.CoreServices.coreservicesd", "文件元数据与目录服务，不是启动服务"),
    ("com.apple.system.DirectoryService.libinfo_v1", "用户与组解析"),
    ("com.apple.system.DirectoryService.membership_v1", "组成员解析"),
    ("com.apple.bsd.dirhelper", "临时目录协助"),
    ("com.apple.mDNSResponder", "DNS 解析"),
    ("com.apple.mDNSResponder.reconfirm", "DNS 重新确认"),
    ("com.apple.SystemConfiguration.configd", "网络配置只读查询"),
    ("com.apple.SystemConfiguration.DNSConfiguration", "DNS 配置"),
    ("com.apple.trustd", "证书信任评估"),
    ("com.apple.trustd.agent", "证书信任评估（用户代理）"),
    ("com.apple.ocspd", "证书吊销检查"),
    ("com.apple.security.syspolicy", "Gatekeeper 策略查询，不读取钥匙串"),
    ("com.apple.fonts", "字体枚举"),
    ("com.apple.FontObjectsServer", "字体对象"),
    ("com.apple.fontserver", "字体服务"),
    ("com.apple.coreservices.quarantine-resolver", "下载隔离属性"),
    ("com.apple.tccd", "隐私权限查询"),
    ("com.apple.tccd.system", "系统隐私权限查询"),
    ("com.apple.hiservices-xpcservice", "区域与输入法服务"),
    ("com.apple.FSEvents", "测试运行器的文件监视"),
    ("com.apple.fseventsd", "文件监视守护进程"),
    ("com.apple.DiskArbitration.diskarbitrationd", "磁盘描述，构建工具会查询"),
    ("com.apple.audio.coreaudiod", "部分运行时初始化会查询音频"),
    ("com.apple.audio.AudioComponentRegistrar", "音频组件枚举"),
    ("com.apple.MobileFileIntegrity", "代码签名查询"),
    ("com.apple.cs.CodeSigningHelper", "代码签名查询"),
    ("com.apple.networkd", "网络状态只读查询"),
];

/// 纵深防御：这些启动器即使用户命令写出来也不能被沙箱执行。不能代替 mach 白名单。
const LAUNCHER_DENY: &[&str] = &["/usr/bin/open", "/usr/bin/osascript", "/bin/launchctl"];

/// 这些 mach 服务名不得出现在白名单里。测试用它锁住边界。
pub const MACH_LOOKUP_FORBIDDEN: &[&str] = &[
    "com.apple.SecurityServer",
    "com.apple.securityd",
    "com.apple.coreservices.launchservicesd",
    "com.apple.lsd.mapdb",
    "com.apple.lsd.modifydb",
    "com.apple.coreservices.appleevents",
    "com.apple.AEServer",
    "com.apple.pasteboard.1",
    "com.apple.pboard",
    "com.apple.xpc.launchd",
];

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
    /// 在允许之后显式写下的拒绝项（后置规则覆盖前序 allow）。
    /// 用来锁住「工作区在本目录内、但绝不能写」的子路径，例如 git 的 config 与 hooks/。
    blocked: Vec<WritablePath>,
    network: NetworkPolicy,
    runtime: PathBuf,
    profile: String,
}

impl CommandSandbox {
    /// 构建沙箱。任何前提不满足都返回 `Err`，由调用方 fail-closed 退回审批路径（R7）。
    pub fn new(
        workspace: &Path,
        writable: Vec<WritablePath>,
        blocked: Vec<WritablePath>,
        network: NetworkPolicy,
        constraints: SandboxConstraints,
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
            canonical_writable.push(canonical_entry(&entry.path, entry.recursive)?);
        }
        let mut canonical_blocked = Vec::with_capacity(blocked.len());
        for entry in blocked {
            canonical_blocked.push(canonical_entry(&entry.path, entry.recursive)?);
        }
        let constraints = normalize_constraints(constraints)?;
        let profile = build_profile(
            &canonical_workspace,
            &canonical_writable,
            &canonical_blocked,
            network,
            &constraints,
        );
        Ok(Self {
            workspace: canonical_workspace,
            writable: canonical_writable,
            blocked: canonical_blocked,
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

    /// 允许之后显式拒绝的条目（如 git config、hooks/）。
    pub fn blocked(&self) -> &[WritablePath] {
        &self.blocked
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
        // M250 R1/R2: nonzero status alone says nothing about Seatbelt.
        // macOS reports denied writes (shell, Git, Python) as EPERM;
        // sandbox-exec also uses this diagnostic when sandbox_apply fails.
        // Do not classify generic EACCES/Permission denied as Seatbelt evidence.
        if stderr.contains("出口代理拒绝") {
            return format!(
                "出口代理拒绝了这次连接：自动放行的命令只能访问软件源白名单。\
                 读取远端请使用 sync_remote；推送、标签、合并或发布请使用 deliver_changes。\
                 本会话工作区仍是 {}。",
                self.workspace.display()
            );
        }
        if !stderr.contains("Operation not permitted") {
            return String::new();
        }
        if stderr.to_ascii_lowercase().contains("network") {
            return format!(
                "沙箱拦截：这条命令的网络超出了自动放行范围（只允许经宿主出口代理访问软件源，以及本机回环）。\
                 读取远端请使用 sync_remote；推送或发布请使用 deliver_changes。工作区 {}。",
                self.workspace.display()
            );
        }
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

    if lower.contains("core.hookspath") && !hooks_path_command_safe(command) {
        return Some(format!(
            "{prefix}：检测到修改 core.hooksPath 或无法安全解析该命令，钩子目录必须保持 .githooks。\
             修复指引：运行 git config core.hooksPath .githooks 恢复，再重试原操作。"
        ));
    }

    None
}

/// Parse shell command boundaries without interpreting expansions. Quoted prose
/// stays data; shell/interpreter input and ambiguous syntax fail closed.
fn hooks_path_command_safe(command: &str) -> bool {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            word.push(c);
            escaped = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && matches!(c, '$' | '`') { return false; }
            if c == q { quote = None; } else { word.push(c); }
            continue;
        }
        match c {
            '\'' | '"' => { quote = Some(c); started = true; }
            // Expansions and heredocs can hide executable commands, not data.
            '`' | '$' | '<' => return false,
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                if started { words.push(std::mem::take(&mut word)); started = false; }
                if !words.is_empty() { commands.push(std::mem::take(&mut words)); }
            }
            c if c.is_whitespace() => {
                if started { words.push(std::mem::take(&mut word)); started = false; }
            }
            _ => { word.push(c); started = true; }
        }
    }
    if quote.is_some() || escaped { return false; }
    if started { words.push(word); }
    if !words.is_empty() { commands.push(words); }
    commands.iter().all(|words| hooks_path_words_safe(words))
}

fn hooks_path_words_safe(words: &[String]) -> bool {
    let lower: Vec<String> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
    // Git's environment configuration is an override even without `git config`.
    if lower.iter().any(|word| word.starts_with("git_config_key_")
        && word.split_once('=').is_some_and(|(_, value)| value == "core.hookspath")) {
        return false;
    }
    let Some(start) = lower.iter().position(|word| !word.contains('=') && word != "env" && word != "command") else {
        return !lower.iter().any(|word| word.contains("core.hookspath"));
    };
    let executable = lower[start].rsplit('/').next().unwrap_or("");
    let args = &lower[start + 1..];
    if matches!(executable, "bash" | "sh" | "zsh" | "dash" | "pwsh" | "powershell") {
        return args.iter().all(|arg| !arg.contains("core.hookspath") || hooks_path_command_safe(arg));
    }
    if executable != "git" {
        // grep/sed/echo arguments are data, not Git options. Other interpreters
        // are not proved safe by this deliberately bounded shell parser.
        return matches!(executable, "grep" | "sed" | "echo" | "printf" | "cat")
            || !args.iter().any(|arg| arg.contains("core.hookspath"));
    }
    for (arg, original) in args.iter().zip(&words[start + 1..]) {
        if let Some((key, _)) = arg.split_once('=') {
            if key.trim_start_matches("-c") == "core.hookspath"
                && !original.split_once('=').is_some_and(|(_, value)| matches!(value, ".githooks" | "./.githooks")) { return false; }
        }
    }
    let Some(config) = args.iter().position(|arg| arg == "config") else {
        for arg in args {
            if arg.contains("core.hookspath") {
                let Some((key, value)) = arg.split_once('=') else { return false; };
                if !matches!(key, "core.hookspath" | "-ccore.hookspath")
                    || !matches!(value, ".githooks" | "./.githooks") { return false; }
            }
        }
        return true;
    };
    let args = &args[config + 1..];
    if !args.iter().any(|arg| arg == "core.hookspath") {
        return !args.iter().any(|arg| arg.contains("core.hookspath"));
    }
    let mut positional = Vec::new();
    let mut read = false;
    let mut write = false;
    for arg in args {
        match arg.as_str() {
            "--get" | "--get-all" | "--get-regexp" | "--get-urlmatch" | "--list" | "-l" => read = true,
            "--unset" | "--unset-all" | "--remove-section" | "--rename-section" => return false,
            "--add" | "--replace-all" => write = true,
            "--global" | "--local" | "--system" | "--worktree" | "--includes" | "--no-includes" | "--show-origin" | "--show-scope" | "--null" | "-z" | "--" => {},
            option if option.starts_with('-') => return false,
            _ => positional.push(arg.as_str()),
        }
    }
    if !read && positional.len() > 1 && positional[0] == "core.hookspath" {
        let originals = &words[start + config + 2..];
        let Some(key) = args.iter().position(|arg| arg == "core.hookspath") else { return false; };
        if !originals.get(key + 1).is_some_and(|value| matches!(value.as_str(), ".githooks" | "./.githooks")) { return false; }
    }
    if read { return !write; }
    match positional.as_slice() {
        ["core.hookspath"] => !write,
        ["core.hookspath", ".githooks" | "./.githooks"] => true,
        _ => false,
    }
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| {
        format!(
            "M224：路径无法规范化（{}）：{error}；策略生成失败即 fail-closed",
            path.display()
        )
    })
}

/// 白名单条目规范化：叶子文件可以还不存在（例如 `packed-refs.lock`、
/// `FETCH_HEAD` 首次创建），这时规范化父目录再拼回文件名——仍然只放行这一个
/// 确切名字，不会把父目录整棵放开。
fn canonical_entry(path: &Path, recursive: bool) -> Result<WritablePath, String> {
    if path.exists() {
        return Ok(WritablePath {
            path: canonical(path)?,
            recursive,
        });
    }
    let parent = path.parent().ok_or_else(|| {
        format!(
            "M224：路径缺少父目录（{}）；策略生成失败即 fail-closed",
            path.display()
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        format!(
            "M224：路径缺少文件名（{}）；策略生成失败即 fail-closed",
            path.display()
        )
    })?;
    Ok(WritablePath {
        path: canonical(parent)?.join(name),
        recursive,
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

fn normalize_constraints(constraints: SandboxConstraints) -> Result<SandboxConstraints, String> {
    if constraints.require_dispatch_deny && constraints.dispatch_dirs.is_empty() {
        return Err(
            "沙箱策略无法确定 dispatch 目录，已拒绝生成放宽后的策略（fail-closed）".into(),
        );
    }
    let mut dispatch_dirs = Vec::with_capacity(constraints.dispatch_dirs.len());
    for path in &constraints.dispatch_dirs {
        dispatch_dirs.push(literal_rule_path(path, "dispatch 目录")?);
    }
    let mut read_deny = Vec::with_capacity(constraints.read_deny.len());
    for path in &constraints.read_deny {
        read_deny.push(literal_rule_path(path, "读拒绝路径")?);
    }
    let mut socket_paths = Vec::with_capacity(constraints.socket_paths.len());
    for path in &constraints.socket_paths {
        socket_paths.push(literal_rule_path(path, "套接字路径")?);
    }
    Ok(SandboxConstraints {
        read_deny,
        dispatch_dirs,
        require_dispatch_deny: constraints.require_dispatch_deny,
        socket_paths,
        denied_loopback_ports: constraints.denied_loopback_ports,
    })
}

/// 存在则规范化；不存在但已是绝对路径则保留字面量。相对路径无法写成稳定规则，直接失败。
fn literal_rule_path(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "沙箱策略无法规范化{label}（{} 不是绝对路径）；策略生成失败即 fail-closed",
            path.display()
        ));
    }
    if path.exists() {
        return canonical(path).map_err(|error| format!("{label}：{error}"));
    }
    Ok(path.to_path_buf())
}

fn build_profile(
    workspace: &Path,
    writable: &[WritablePath],
    blocked: &[WritablePath],
    network: NetworkPolicy,
    constraints: &SandboxConstraints,
) -> String {
    let mut profile = String::new();
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n");
    profile.push_str("(allow process*)\n");
    for launcher in LAUNCHER_DENY {
        profile.push_str(&format!(
            "(deny process-exec (literal \"{launcher}\"))\n"
        ));
    }
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow mach-lookup\n");
    for (name, _reason) in MACH_LOOKUP_ALLOW {
        profile.push_str(&format!("    (global-name \"{name}\")\n"));
    }
    profile.push_str(")\n");
    // 构建/测试工具会结束自己的子进程（vitest worker、cargo 子任务），也会用
    // POSIX 信号量做 worker 协调。信号只放行到**同一沙箱内**的目标：子进程继承
    // 沙箱，所以够用；而同一用户下沙箱外的进程（编辑器、其它会话、系统服务）
    // 仍然发不到——实测 `(allow signal)` 过宽、`(target pgrp self)` 能杀掉沙箱
    // 外进程，两者都不可接受。
    profile.push_str("(allow signal (target same-sandbox))\n");
    profile.push_str("(allow ipc-posix-sem)\n");
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
    // 后置拒绝覆盖前序允许：白名单条目覆盖到的子路径也可以是禁区（git config / hooks）。
    for entry in blocked {
        let literal = escape(&entry.path.to_string_lossy());
        profile.push_str(&format!("(deny file-write* (literal \"{literal}\"))\n"));
        if entry.recursive {
            profile.push_str(&format!("(deny file-write* (subpath \"{literal}\"))\n"));
        }
    }
    // 读拒绝写在写白名单之后：Seatbelt 后匹配者生效，这些路径即使落在某个写规则附近也不可读。
    for path in constraints
        .read_deny
        .iter()
        .chain(constraints.dispatch_dirs.iter())
    {
        let literal = escape(&path.to_string_lossy());
        profile.push_str(&format!("(deny file-read* (literal \"{literal}\"))\n"));
        profile.push_str(&format!("(deny file-read* (subpath \"{literal}\"))\n"));
    }
    match network {
        NetworkPolicy::Disabled => {}
        NetworkPolicy::Full => profile.push_str("(allow network*)\n"),
        NetworkPolicy::RegistryProxy { port } => {
            // 先拒绝全部网络，再放行回环，最后按端口和套接字收紧。后写的规则覆盖先写的规则。
            // sandbox-exec 的 ip/tcp 主机只接受 `*` 或 `localhost`。写 `127.0.0.1` 会在
            // 加载配置时直接失败（host must be * or localhost），命令根本进不了沙箱。
            // 代理绑定的 127.0.0.1 就是 localhost 这个回环名。
            profile.push_str("(deny network*)\n");
            profile.push_str("(allow network-bind (local ip \"localhost:*\"))\n");
            profile.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
            profile.push_str("(allow network-outbound (remote ip \"localhost:*\"))\n");
            profile.push_str(&format!(
                "(allow network-outbound (remote tcp \"localhost:{port}\"))\n"
            ));
            for denied in &constraints.denied_loopback_ports {
                profile.push_str(&format!(
                    "(deny network-outbound (remote tcp \"localhost:{denied}\"))\n"
                ));
                profile.push_str(&format!(
                    "(deny network-outbound (remote ip \"localhost:{denied}\"))\n"
                ));
            }
            for socket in &constraints.socket_paths {
                let literal = escape(&socket.to_string_lossy());
                profile.push_str(&format!(
                    "(deny network-outbound (remote unix-socket (path-literal \"{literal}\")))\n"
                ));
            }
        }
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
    fn m245_read_only_hooks_path_is_not_bypass() {
        for command in [
            "git config --get core.hooksPath",
            "git config --get-all core.hooksPath",
            "git config core.hooksPath",
            "git config --list",
            "grep core.hooksPath AGENTS.md",
            "sed -n '/core.hooksPath/p' AGENTS.md",
            "echo 'git config --unset core.hooksPath'",
            "git config --get core.hooksPath; git status",
            "(git config --get core.hooksPath)",
            "git config --get core.hooksPath | cat",
            "git config --get core.hooksPath && git config --get-all core.hooksPath",
            "git config core.hooksPath '.githooks'",
        ] {
            assert!(gate_bypass(command).is_none(), "read-only command: {command}");
        }
    }

    #[test]
    fn m245_hooks_path_writes_and_compounds_are_blocked() {
        for command in [
            "git config core.hooksPath /tmp/x",
            "git config --unset core.hooksPath",
            "git config --unset-all core.hooksPath",
            "git config --global core.hooksPath /tmp/x",
            "git config --local --replace-all core.hooksPath /tmp/x",
            "git -c core.hooksPath=/dev/null commit",
            "git -c core.hooksPath= commit",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/tmp/x git commit",
            "git config --get core.hooksPath; git config --unset core.hooksPath",
            "git config --get core.hooksPath && git config --unset core.hooksPath",
            "git config --get core.hooksPath || git config --unset core.hooksPath",
            "git config --get core.hooksPath | git config --unset core.hooksPath",
            "git config --get core.hooksPath\ngit config --unset core.hooksPath",
            "(git config --get core.hooksPath; git config --unset core.hooksPath)",
            "git config core.hooksPath .githooks; git config --unset core.hooksPath",
            "git config core.hooksPath .githooks; git -c core.hooksPath= commit",
            "bash -c 'git config --unset core.hooksPath'",
            "echo $(git config --unset core.hooksPath)",
            "git config --get 'core.hooksPath",
            "git config core.hooksPath \"$HOOKS\"",
            "git config --no-such-option core.hooksPath",
            "git --config-env=core.hooksPath=SYNTHETIC commit",
            "git config core.hooksPath .GITHOOKS",
            "git -c core.hooksPath=.GITHOOKS commit",
            "git config set core.hooksPath /tmp/x",
            "echo \"$(git config --unset core.hooksPath)\"",
            "git commit --no-verify -m x",
            "CODEFACTORY_SKIP_SYNC_GATE=1 git commit -m x",
        ] {
            assert!(gate_bypass(command).is_some(), "unsafe command: {command}");
        }
    }

    #[test]
    fn m245_generated_command_boundaries_never_hide_mutations() {
        let reads = ["git config --get core.hooksPath", "grep core.hooksPath AGENTS.md", "git config core.hooksPath .githooks"];
        let writes = ["git config --unset core.hooksPath", "git -c core.hooksPath= commit", "git config --global core.hooksPath /tmp/synthetic"];
        for read in reads {
            for write in writes {
                for separator in [";", "&&", "||", "|", "\n"] {
                    for command in [format!("{read} {separator} {write}"), format!("({write}) {separator} {read}")] {
                        assert!(gate_bypass(&command).is_some(), "{command}");
                    }
                }
            }
        }
    }

    #[test]
    fn registry_proxy_profile_names_localhost_and_not_a_numeric_address() {
        let mut constraints = SandboxConstraints::default();
        constraints.denied_loopback_ports.push(1);
        let profile = build_profile(
            Path::new("/private/var/ws"),
            &[],
            &[],
            NetworkPolicy::RegistryProxy { port: 9 },
            &constraints,
        );
        assert!(!profile.contains("127.0.0.1"), "{profile}");
        assert!(profile.contains("(allow network-outbound (remote ip \"localhost:*\"))"));
        assert!(profile.contains("(allow network-outbound (remote tcp \"localhost:9\"))"));
        assert!(profile.contains("(deny network-outbound (remote tcp \"localhost:1\"))"));
    }

    #[test]
    fn profile_keeps_writes_denied_by_default() {
        let profile = build_profile(
            Path::new("/private/var/ws"),
            &[],
            &[],
            NetworkPolicy::Full,
            &SandboxConstraints::default(),
        );
        assert!(profile.contains("(deny file-write*)"));
        assert!(profile.contains("(allow file-write* (subpath \"/private/var/ws\"))"));
        assert!(profile.contains("(allow network*)"));
        assert!(profile.contains("(deny process-exec (literal \"/usr/bin/open\"))"));
        assert!(profile.contains("com.apple.trustd"));
        assert!(!profile.contains("com.apple.SecurityServer"));
        assert!(!profile.contains("(allow mach-lookup)\n"));
        assert!(
            profile.contains("(allow signal (target same-sandbox))"),
            "{profile}"
        );
        assert!(
            !profile.contains("(allow signal)\n"),
            "signal 不得无目标放行（会给同用户任意进程发信号）: {profile}"
        );
        assert!(profile.contains("(allow ipc-posix-sem)"), "{profile}");
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
