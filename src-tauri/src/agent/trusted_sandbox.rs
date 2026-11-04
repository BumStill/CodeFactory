//! M224：信任模式（`SandboxMode::Off` + 全量权限）下的系统沙箱接入点。
//!
//! 这一层只做三件事：
//! 1. 由设置决定沙箱是否启用（R9 回退开关），默认开启；
//! 2. 把「会话工作区 + 少量编译/临时目录」翻译成写入白名单，并构建 `CommandSandbox`；
//! 3. 构建失败或运行时缺失时返回 `FallbackApproval`，让权限网关退回原有审批路径（R7），
//!    绝不静默执行无沙箱命令。
//!
//! 前台 `bash` 工具与 `background_task_start` 必须共用这里的 `wrap_command`，
//! 否则两个启动点会出现「一个在沙箱内、一个在沙箱外」的分叉。

use std::path::{Path, PathBuf};

use crate::command_sandbox::{CommandSandbox, CommandLaunch, NetworkPolicy, WritablePath};
use crate::config::settings::Settings;

/// 沙箱的最终决定：要么真的约束进程，要么退回审批。
pub enum SandboxGate {
    /// 沙箱已就绪，命令必须以这个实例启动。
    Enforced(CommandSandbox),
    /// 沙箱不可用/被关闭：退回审批路径，并带上模型可读的原因。
    FallbackApproval(String),
}

/// 磁盘上的开关值（后台任务启动点没有 Settings 句柄，只能读同一份配置）。
fn setting_from_disk() -> Option<bool> {
    let text = std::fs::read_to_string(crate::config::settings::config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("trusted_command_sandbox").and_then(|v| v.as_bool())
}

/// R9：回退开关。有 Settings 句柄时以它为准；没有时读配置文件；都读不到按默认开启，
/// 因为"读不到配置"不等于"用户关掉了沙箱"。
pub fn enabled(settings: Option<&Settings>) -> bool {
    match settings {
        Some(settings) => settings.trusted_command_sandbox,
        None => setting_from_disk().unwrap_or(true),
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// 写入白名单：工作区整棵树 + 编译/临时目录。
///
/// 这里刻意**不**包含 `$HOME`：读可以放开，写只放开工作区与构建缓存，
/// 这样 `echo > ~/outside.txt` 会被系统拒绝而不是靠解析命令文字判断。
pub fn writable_paths(workspace: &Path) -> Vec<WritablePath> {
    let mut paths = vec![WritablePath::tree(workspace)];

    let mut extras: Vec<PathBuf> = Vec::new();
    extras.push(std::env::temp_dir());
    for var in ["CARGO_HOME", "RUSTUP_HOME", "TMPDIR"] {
        if let Some(path) = env_path(var) {
            extras.push(path);
        }
    }
    if let Some(home) = env_path("HOME") {
        for relative in [".cargo", ".rustup", ".npm", ".cache", "Library/Caches"] {
            extras.push(home.join(relative));
        }
    }
    // 共享编译缓存可能落在工作区之外（worktree 场景），存在才加入。
    if let Some(cache) = env_path("CODEFACTORY_BUILD_CACHE_ROOT") {
        extras.push(cache);
    }

    for path in extras {
        if path.is_absolute() {
            paths.push(WritablePath::tree(path));
        }
    }
    paths
}

/// 解析沙箱。失败一律是 `FallbackApproval`，调用方据此退回审批。
pub fn resolve(workspace: &Path, settings: Option<&Settings>) -> SandboxGate {
    if !enabled(settings) {
        return SandboxGate::FallbackApproval(
            "M224：信任模式沙箱已被设置关闭（trusted_command_sandbox=false），命令按原审批路径执行。"
                .to_string(),
        );
    }
    let absolute = if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        return SandboxGate::FallbackApproval(format!(
            "M224：工作区路径不是绝对路径（{}），无法建立沙箱边界；命令按原审批路径执行。",
            workspace.display()
        ));
    };
    match CommandSandbox::new(
        &absolute,
        writable_paths(&absolute),
        NetworkPolicy::Enabled,
    ) {
        Ok(sandbox) => SandboxGate::Enforced(sandbox),
        Err(reason) => SandboxGate::FallbackApproval(reason),
    }
}

/// R5：权限网关用的廉价判定——沙箱此刻是否真的会约束命令。
pub fn is_enforced(workspace: &Path, settings: Option<&Settings>) -> bool {
    matches!(resolve(workspace, settings), SandboxGate::Enforced(_))
}

/// 供诊断/账本使用的状态文本。
pub fn describe(workspace: &Path, settings: Option<&Settings>) -> String {
    match resolve(workspace, settings) {
        SandboxGate::Enforced(sandbox) => format!(
            "强制执行：{}（工作区 {}，写入白名单 {} 项）",
            sandbox.runtime().display(),
            sandbox.workspace().display(),
            sandbox.writable().len()
        ),
        SandboxGate::FallbackApproval(reason) => format!("退回审批：{reason}"),
    }
}

/// 把一次启动包进沙箱。返回 `(program, args, sandbox)`；`sandbox` 为 `Some` 时
/// 调用方必须在失败输出里带上 `failure_hint`（R8）。
pub fn wrap_command(
    workspace: &Path,
    settings: Option<&Settings>,
    program: &str,
    args: &[String],
) -> (String, Vec<String>, Option<CommandSandbox>) {
    match resolve(workspace, settings) {
        SandboxGate::Enforced(sandbox) => {
            let CommandLaunch {
                program: wrapped_program,
                args: wrapped_args,
            } = sandbox.wrap(program, args);
            (wrapped_program, wrapped_args, Some(sandbox))
        }
        SandboxGate::FallbackApproval(_) => (program.to_string(), args.to_vec(), None),
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn temp_dir(tag: &str) -> PathBuf {
        // 目录名只能是安全的 shell/profile 字符：括号会破坏 sh -c 与 profile 字面量。
        let dir = std::env::temp_dir().join(format!("m224-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 真正的"工作区外"：临时目录整棵树在写入白名单里（构建缓存需要），
    /// 所以边界探针必须落在 HOME 根目录这种未被允许的位置。
    fn outside_file(tag: &str) -> PathBuf {
        let home = std::env::var("HOME").expect("HOME");
        let path = PathBuf::from(home).join(format!("m224-denied-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn sandbox(workspace: &Path) -> CommandSandbox {
        CommandSandbox::new(
            workspace,
            writable_paths(workspace),
            NetworkPolicy::Enabled,
        )
        .expect("sandbox builds on macOS")
    }

    /// 生成的 profile 必须真的表达三条边界：写白名单、工作区写入、网络出站。
    #[test]
    fn profile_expresses_boundaries_and_escapes_workspace() {
        let dir = temp_dir("profile");
        let odd = dir.join("with space and \"quote\"");
        std::fs::create_dir_all(&odd).expect("odd dir");
        let sandbox = sandbox(&odd);
        let launch = sandbox.wrap("bash", &["-lc".into(), "printf ok".into()]);
        assert_eq!(launch.program, crate::command_sandbox::MACOS_SANDBOX_EXEC);
        assert_eq!(launch.args.first().map(String::as_str), Some("-p"));
        let profile = launch.args.get(1).expect("profile arg");
        assert!(profile.contains("(deny file-write*)"), "{profile}");
        assert!(profile.contains("(deny default)"), "{profile}");
        assert!(profile.contains("(allow network*)"), "{profile}");
        assert!(profile.contains("allow file-write*"), "{profile}");
        // 路径里的空格与引号必须被转义，否则 profile 会解析失败或越界。
        assert!(profile.contains("\\\""), "{profile}");
        assert_eq!(
            launch.args[2..].to_vec(),
            vec!["bash".to_string(), "-lc".to_string(), "printf ok".to_string()]
        );
        assert!(!sandbox.failure_hint("").trim().is_empty());
    }

    /// 真实执行：工作区内写成功，工作区外写被系统拒绝，且提示可读（R5/R8）。
    #[test]
    fn real_spawn_allows_inside_and_denies_outside() {
        let workspace = temp_dir("inside");
        let sandbox = sandbox(&workspace);

        let inside_file = workspace.join("inside.txt");
        let inside_launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                format!("printf hi > {}", inside_file.display()),
            ],
        );
        let inside = Command::new(&inside_launch.program)
            .args(&inside_launch.args)
            .current_dir(&workspace)
            .output()
            .expect("inside spawn");
        assert!(
            inside.status.success(),
            "inside write must succeed: {}",
            String::from_utf8_lossy(&inside.stderr)
        );
        assert_eq!(std::fs::read_to_string(&inside_file).unwrap_or_default(), "hi");

        let outside_file = outside_file("spawn");
        let outside_launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                format!("printf hi > {}", outside_file.display()),
            ],
        );
        let denied = Command::new(&outside_launch.program)
            .args(&outside_launch.args)
            .current_dir(&workspace)
            .output()
            .expect("outside spawn");
        assert!(!denied.status.success(), "outside write must be denied");
        assert!(!outside_file.exists(), "outside file must not be created");
        let stderr = String::from_utf8_lossy(&denied.stderr);
        let lower = stderr.to_lowercase();
        assert!(
            lower.contains("operation not permitted") || lower.contains("deny"),
            "expected a sandbox veto, got: {stderr}"
        );
        let hint = sandbox.failure_hint(&stderr);
        assert!(hint.contains("沙箱") || hint.contains("工作区"), "{hint}");
    }

    /// `-f` 文件模式与 `-p` 内联模式必须等价（review 里两种调用都有背书）。
    #[test]
    fn profile_file_mode_runs_and_enforces() {
        let workspace = temp_dir("filemode");
        let sandbox = sandbox(&workspace);
        let profile_path = workspace.join("m224.sb");
        std::fs::write(&profile_path, sandbox.profile()).expect("write profile");

        let ok = Command::new(crate::command_sandbox::MACOS_SANDBOX_EXEC)
            .args(["-f", profile_path.to_str().unwrap()])
            .args(["/bin/sh", "-c", "printf ok"])
            .current_dir(&workspace)
            .output()
            .expect("file mode spawn");
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "ok");

        let outside_file = outside_file("filemode");
        let denied = Command::new(crate::command_sandbox::MACOS_SANDBOX_EXEC)
            .args(["-f", profile_path.to_str().unwrap()])
            .args([
                "/bin/sh",
                "-c",
                &format!("printf hi > {}", outside_file.display()),
            ])
            .current_dir(&workspace)
            .output()
            .expect("file mode denied spawn");
        assert!(!denied.status.success(), "file mode must deny outside write");
        assert!(!outside_file.exists());
    }

    /// 每次真实启动都有可观察的进程证据：spawn 前 0 个匹配，运行中至少 1 个。
    #[test]
    fn spawn_is_observable_before_and_after() {
        let workspace = temp_dir("spawncount");
        let sandbox = sandbox(&workspace);
        let token = format!("m224spawnprobe{}", std::process::id());
        let launch = sandbox.wrap(
            "/bin/sh",
            &["-c".into(), format!("sleep 0.6; printf 'done {token}'")],
        );

        fn matching(token: &str) -> usize {
            let out = Command::new("ps")
                .args(["-Ao", "command"])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| line.contains(token))
                .count()
        }

        assert_eq!(matching(&token), 0, "no process should match before spawn");
        let mut child = Command::new(&launch.program)
            .args(&launch.args)
            .current_dir(&workspace)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut observed = 0usize;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            observed = observed.max(matching(&token));
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let output = child.wait_with_output().expect("wait");
        assert!(observed >= 1, "sandboxed process must be observable while running");
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains(&token));
    }
}
