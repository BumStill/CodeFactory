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
    // 编译缓存：任务专属缓存目录与 .codefactory-cache 容器（沙箱化构建必须能写，
    // 否则 R10 矩阵里的 cargo/pnpm 会因缓存只读而失败）。
    let task_cache = crate::build_cache::task_cache_root(workspace);
    if let Some(container) = task_cache.parent() {
        extras.push(container.to_path_buf());
    }
    extras.push(task_cache);
    if let Some(cache) = env_path("CODEFACTORY_BUILD_CACHE_ROOT") {
        extras.push(cache);
    }

    // 只登记真实存在的目录：不存在的路径无法规范化，而 fail-closed 会让整个
    // 沙箱构建失败（构建缓存目录由构建层在需要时创建）。
    for path in extras {
        if path.is_absolute() && path.exists() && !paths.iter().any(|p| p.path() == path.as_path()) {
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

/// R5：沙箱生效时，本地普通命令不再因为「无法用命令文字证明访问范围」
/// 进入审批——范围由内核保证。破坏性改写与远端动作不在豁免范围内。
pub fn covers_local_command(
    enforced: bool,
    destructive_rewrite: bool,
    remote: bool,
    outside_scope: bool,
) -> bool {
    enforced && !destructive_rewrite && !remote && !outside_scope
}

/// R7：沙箱不可用时，命令仍按既有审批路径执行，但结果里必须写明这次没有沙箱，
/// 不能静默降级。用户主动关闭时同样只提示一次，不阻断执行。
pub fn fallback_notice(workspace: &Path, settings: Option<&Settings>) -> Option<String> {
    match resolve(workspace, settings) {
        SandboxGate::Enforced(_) => None,
        SandboxGate::FallbackApproval(reason) => Some(if enabled(settings) {
            format!(
                "M224：系统沙箱不可用（{reason}）；本命令按既有审批路径在本机执行，没有系统沙箱约束。"
            )
        } else {
            "M224：可信模式沙箱已按设置关闭（trusted_command_sandbox=false），本命令在本机无沙箱执行。".to_string()
        }),
    }
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

    /// R5：只有"沙箱生效 + 非破坏性改写 + 非远端"的本地命令才免审批。
    #[test]
    fn local_command_exemption_requires_a_live_sandbox_and_no_destructive_intent() {
        assert!(covers_local_command(true, false, false, false));
        assert!(
            !covers_local_command(false, false, false, false),
            "无沙箱必须退回审批"
        );
        assert!(
            !covers_local_command(true, true, false, false),
            "破坏性改写仍要审批"
        );
        assert!(
            !covers_local_command(true, false, true, false),
            "远端动作仍走交付裁决"
        );
        assert!(
            !covers_local_command(true, false, false, true),
            "凭据/远端写/跨 checkout 不在沙箱覆盖范围内"
        );
    }

    /// 沙箱覆盖不到的风险必须仍然被识别出来（否则 R5 会变成凭据外带的白名单）。
    #[test]
    fn credential_remote_and_cross_checkout_commands_stay_out_of_scope() {
        for command in [
            "gh auth token",
            "gh pr merge 7 --squash",
            "git push --force origin main",
            "git push origin :main",
            "git config --global core.hooksPath /tmp/hooks",
            "git checkout -- ../other",
            "git reset --hard HEAD && rm -rf ../other",
            "curl -s https://example.com",
        ] {
            assert!(
                crate::agent::trusted_git::outside_write_sandbox_scope(command),
                "{command} must stay inside approval"
            );
        }
        for command in [
            "git status --porcelain",
            "git add -A && git commit -m 'x'",
            "git fetch --prune origin main && git merge origin/main --no-edit",
            "python3 - <<'PY'\nopen('tracked.txt','w').write('x')\nPY",
        ] {
            assert!(
                !crate::agent::trusted_git::outside_write_sandbox_scope(command),
                "{command} is local-write scope"
            );
        }
    }

    /// R7：沙箱不可用时必须有明确、非静默的说明；生效时不产生噪音。
    #[test]
    fn fallback_notice_is_explicit_and_absent_when_enforced() {
        let workspace = temp_dir("notice");
        assert!(
            fallback_notice(&workspace, None).is_none(),
            "沙箱生效时不应提示降级"
        );
        let relative = Path::new("relative/workspace");
        let notice = fallback_notice(relative, None).expect("相对路径必须 fail-closed");
        assert!(notice.contains("系统沙箱不可用"), "{notice}");
        assert!(notice.contains("没有系统沙箱约束"), "{notice}");
    }

    /// R8：失败提示必须同时给出"被拒路径"和"登记工作区"，模型才能自我修复。
    #[test]
    fn failure_hint_names_the_path_and_the_registered_workspace() {
        let workspace = temp_dir("hint");
        let sandbox = sandbox(&workspace);
        let hint = sandbox.failure_hint(
            "/bin/sh: /Users/someone/outside.txt: Operation not permitted",
        );
        assert!(hint.contains("/Users/someone/outside.txt"), "{hint}");
        let canonical = workspace.canonicalize().unwrap_or(workspace.clone());
        assert!(
            hint.contains(&canonical.display().to_string()),
            "hint must name the registered workspace: {hint}"
        );
    }

    /// R10：把本仓库工作区的 profile 落盘，供兼容矩阵（`tools/verify/m224-r10-matrix.mjs`）
    /// 用与产品完全相同的策略执行真实工作流。
    #[test]
    fn emits_repo_profile_for_the_r10_matrix() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();
        let sandbox = sandbox(&repo);
        let dir = repo.join("target").join("m224");
        std::fs::create_dir_all(&dir).expect("target/m224");
        let path = dir.join("repo.sb");
        std::fs::write(&path, sandbox.profile()).expect("write repo profile");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(written.contains("(deny file-write*)"), "{written}");
        assert!(
            written.contains(&repo.canonicalize().unwrap_or(repo.clone()).display().to_string()),
            "profile must whitelist the repository workspace"
        );
        println!("m224 profile: {}", path.display());
    }

    /// 沙箱内的信号不能越界：对沙箱外（同一用户）的进程发信号必须失败，
    /// 且那个进程要活着。这是 R3/R4 之外最容易漏的一条边界。
    #[test]
    fn signals_cannot_escape_the_sandbox() {
        let workspace = temp_dir("signal-outside");
        let sandbox = sandbox(&workspace);

        let mut outsider = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("spawn outsider");
        let outsider_pid = outsider.id();

        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                format!("kill -TERM {outsider_pid}; echo outside=$?"),
            ],
        );
        let output = Command::new(&launch.program)
            .args(&launch.args)
            .output()
            .expect("run kill");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            stdout.contains("outside=1"),
            "kill of an out-of-sandbox process must fail: {stdout}{stderr}"
        );

        std::thread::sleep(Duration::from_millis(200));
        assert!(
            outsider.try_wait().expect("try_wait").is_none(),
            "the out-of-sandbox process must still be alive"
        );
        let _ = outsider.kill();
        let _ = outsider.wait();
    }

    /// 同一沙箱内的子进程必须还能被结束，否则构建/测试工具会挂死。
    #[test]
    fn signals_reach_processes_inside_the_same_sandbox() {
        let workspace = temp_dir("signal-inside");
        let sandbox = sandbox(&workspace);
        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                "sleep 30 & child=$!; kill -TERM $child; echo inside=$?".into(),
            ],
        );
        let output = Command::new(&launch.program)
            .args(&launch.args)
            .output()
            .expect("run kill inside");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            stdout.contains("inside=0"),
            "same-sandbox kill must succeed: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 每次真实启动都有可观察的进程证据。macOS 沙箱拒绝 `ps` 读进程表
    /// （seatbelt 不授予该能力），所以可观察性由"运行中写下的心跳 + 退出后的完成标记"
    /// 证明：两者都出现，说明确实有一个进程带着这份 profile 跑过。
    #[test]
    fn spawn_is_observable_before_and_after() {
        let workspace = temp_dir("spawncount");
        let sandbox = sandbox(&workspace);
        let started = workspace.join("started.txt");
        let done = workspace.join("done.txt");
        let token = format!("m224spawnprobe{}", std::process::id());
        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                format!(
                    "printf '{token}' > '{}'; sleep 0.6; printf '{token}' > '{}'",
                    started.display(),
                    done.display()
                ),
            ],
        );

        assert!(!started.exists(), "nothing should have run before the spawn");
        let mut child = Command::new(&launch.program)
            .args(&launch.args)
            .current_dir(&workspace)
            .spawn()
            .expect("spawn");

        let mut observed_while_running = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if std::fs::read_to_string(&started).map(|s| s == token).unwrap_or(false) {
                observed_while_running = true;
            }
            if child.try_wait().expect("try_wait").is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = child.wait().expect("wait");
        assert!(
            observed_while_running,
            "the sandboxed process must be observable while it is running"
        );
        assert!(status.success(), "sandboxed spawn must exit cleanly");
        assert_eq!(
            std::fs::read_to_string(&done).expect("completion marker"),
            token
        );
    }
}
