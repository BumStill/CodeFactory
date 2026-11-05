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
use crate::util::no_window::NoWindow;
use crate::config::settings::Settings;

/// macOS 拒绝嵌套沙箱的判定探针。
///
/// 沙箱内再调 `sandbox-exec` 只会得到 `sandbox_apply: Operation not permitted`。
/// 会话（agent）本身跑在沙箱里时，再包一层会把每条命令和整批测试变成假失败，
/// 所以这里用一次真实探测判定，而不是靠环境变量猜。
pub fn sandbox_application_refused() -> bool {
    static REFUSED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *REFUSED.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            match std::process::Command::new(crate::command_sandbox::MACOS_SANDBOX_EXEC)
                .no_window()
                .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
                .output()
            {
                Ok(output) => {
                    !output.status.success()
                        && String::from_utf8_lossy(&output.stderr).contains("sandbox_apply")
                }
                // 运行时缺失属于另一条失败路径（R7），不当作嵌套沙箱。
                Err(_) => false,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    })
}

/// 沙箱的最终决定：要么真的约束进程，要么退回审批。
pub enum SandboxGate {
    /// 沙箱已就绪，命令必须以这个实例启动。
    Enforced(CommandSandbox),
    /// 当前进程本身已在系统沙箱内（macOS 不允许嵌套）：命令不再二次包装，
    /// 约束由外层沙箱提供；这不是"没有沙箱"，所以不能按 R7 的降级文案报。
    Inherited(String),
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
    sandbox_paths(workspace).0
}

/// 允许之后仍需显式拒绝的路径（当前只有 git 的 config 与 hooks/）。
pub fn blocked_paths(workspace: &Path) -> Vec<WritablePath> {
    sandbox_paths(workspace).1
}

/// 一次算清允许与拒绝两套路径，避免 `resolve` 里重复读 git 元数据。
pub fn sandbox_paths(workspace: &Path) -> (Vec<WritablePath>, Vec<WritablePath>) {
    let mut paths: Vec<WritablePath> = vec![WritablePath::tree(workspace)];
    let mut extras: Vec<WritablePath> = vec![WritablePath::tree(std::env::temp_dir())];

    // 收窄 ~/.cargo：只放行 cargo 真正要写的 `registry` 与 `git` 两个子树，外加包缓存
    // 锁文件。`~/.cargo/bin` 与 `~/.cargo/config.toml` 一旦可写，沙箱内进程就能植入
    // 「下次在沙箱外运行」的可执行文件，或改掉 cargo 配置去加载外部命令。
    let cargo_home = env_path("CARGO_HOME")
        .or_else(|| env_path("HOME").map(|home| home.join(".cargo")));
    if let Some(home) = cargo_home {
        extras.push(WritablePath::tree(home.join("registry")));
        extras.push(WritablePath::tree(home.join("git")));
        extras.push(WritablePath::literal(home.join(".package-cache")));
    }
    // `~/.rustup` 刻意不放行：cargo 构建只**读** toolchain，让 toolchain 目录可写等于
    // 允许替换编译器；实测缺省不需要写（R10 矩阵在沙箱内 cargo test 通过）。
    if let Some(home) = env_path("HOME") {
        for relative in [".npm", ".cache", "Library/Caches"] {
            extras.push(WritablePath::tree(home.join(relative)));
        }
    }
    if let Some(tmpdir) = env_path("TMPDIR") {
        extras.push(WritablePath::tree(tmpdir));
    }
    // 编译缓存：任务专属缓存目录与 .codefactory-cache 容器（沙箱化构建必须能写，
    // 否则 R10 矩阵里的 cargo/pnpm 会因缓存只读而失败）。
    let task_cache = crate::build_cache::task_cache_root(workspace);
    if let Some(container) = task_cache.parent() {
        extras.push(WritablePath::tree(container.to_path_buf()));
    }
    extras.push(WritablePath::tree(task_cache));
    if let Some(cache) = env_path("CODEFACTORY_BUILD_CACHE_ROOT") {
        extras.push(WritablePath::tree(cache));
    }

    let (git_allow, git_block) = git_paths(workspace);
    extras.extend(git_allow);

    // 只登记真实存在的目录：不存在的路径无法规范化，而 fail-closed 会让整个
    // 沙箱构建失败（构建缓存目录由构建层在需要时创建）。单文件条目（literal）允许
    // 尚不存在——`FETCH_HEAD`、`packed-refs.lock` 都是首次写入时才创建。
    for entry in extras {
        if !entry.path().is_absolute() {
            continue;
        }
        if entry.is_recursive() && !entry.path().exists() {
            // 父目录存在就保留规则：cargo 的 `registry`/`git` 子树是首次构建时才创建的，
            // 规则本身是窄路径，提前登记不会放宽边界。父目录也不存在时才丢弃
            // （构建缓存容器由构建层按需创建，缺了会让规范化失败）。
            let parent_exists = entry
                .path()
                .parent()
                .map(|parent| parent.exists())
                .unwrap_or(false);
            if !parent_exists {
                continue;
            }
        }
        if paths
            .iter()
            .any(|p| p.path() == entry.path() && p.is_recursive() == entry.is_recursive())
        {
            continue;
        }
        paths.push(entry);
    }
    (paths, git_block)
}

/// R3：git 元数据的写入面。
///
/// 真实工作区往往是 linked worktree：`.git` 是一个文件，内容指向
/// `<common>/worktrees/<id>`，index、HEAD、logs 全在工作区之外。沙箱一旦生效，
/// 不放行这些路径就会让会话里的 `git add` / `commit` / `merge` 全部失败。
/// 反过来，config 与 hooks/ 必须永远不可写——那等于允许沙箱内进程植入"下次在
/// 沙箱外执行"的钩子。
fn git_paths(workspace: &Path) -> (Vec<WritablePath>, Vec<WritablePath>) {
    let Some((git_dir, common)) = git_dirs(workspace) else {
        return (Vec::new(), Vec::new());
    };
    let mut allow = Vec::new();
    let mut block = vec![
        WritablePath::literal(common.join("config")),
        WritablePath::literal(common.join("config.worktree")),
        WritablePath::tree(common.join("hooks")),
    ];

    if git_dir == common {
        // 普通仓库：index/HEAD/index.lock 就在这个目录根上，必须整棵放行，
        // 再由上面的后置 deny 把 config 与 hooks/ 锁死。
        allow.push(WritablePath::tree(common));
    } else {
        // linked worktree：本工作区的元数据目录整棵放行（index、HEAD、logs/HEAD、
        // COMMIT_EDITMSG、index.lock 都在里面）。
        allow.push(WritablePath::tree(git_dir));
        for relative in ["objects", "refs", "logs", "worktrees", "branches", "modules"] {
            allow.push(WritablePath::tree(common.join(relative)));
        }
        for relative in [
            "packed-refs",
            "packed-refs.lock",
            "FETCH_HEAD",
            "ORIG_HEAD",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "shallow",
            "gc.log",
        ] {
            allow.push(WritablePath::literal(common.join(relative)));
        }
    }
    block.retain(|entry| entry.is_recursive() || entry.path().parent().is_some());
    (allow, block)
}

/// `git rev-parse --git-dir --git-common-dir`，一次性拿到本工作区与公共仓库目录。
fn git_dirs(workspace: &Path) -> Option<(PathBuf, PathBuf)> {
    let output = std::process::Command::new("git")
        .no_window()
        .arg("-C")
        .arg(workspace)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let git_dir = PathBuf::from(lines.next()?);
    let common = PathBuf::from(lines.next()?);
    Some((git_dir, common))
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
    // macOS 不允许嵌套沙箱：进程已经在沙箱里时再包一层只会拿到
    // `sandbox_apply: Operation not permitted`，把每条命令变成假失败。
    // 这不是"没有沙箱"，命令仍受外层策略约束，因此单列 Inherited。
    if sandbox_application_refused() {
        return SandboxGate::Inherited(
            "M224：当前进程已在系统沙箱内（macOS 不允许嵌套沙箱），命令继承外层沙箱约束执行。"
                .to_string(),
        );
    }
    let (writable, blocked) = sandbox_paths(&absolute);
    match CommandSandbox::new(&absolute, writable, blocked, NetworkPolicy::Enabled) {
        Ok(sandbox) => SandboxGate::Enforced(sandbox),
        Err(reason) => SandboxGate::FallbackApproval(reason),
    }
}

/// R5：权限网关用的廉价判定——沙箱此刻是否真的会约束命令。
pub fn is_enforced(workspace: &Path, settings: Option<&Settings>) -> bool {
    // Inherited 不算"已强制"：外层策略不是我们生成的，无法据此免审批，
    // 只能把"继承外层沙箱"如实写进提示（R7 的 fail-closed 语义保持不变）。
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
        SandboxGate::Inherited(reason) => Some(format!("{reason}未叠加第二层策略。")),
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
        SandboxGate::Inherited(reason) => format!("继承外层沙箱：{reason}"),
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
        SandboxGate::Inherited(_) | SandboxGate::FallbackApproval(_) => {
            (program.to_string(), args.to_vec(), None)
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// macOS 不允许嵌套沙箱：沙箱内再调 sandbox-exec 会得到
    /// `sandbox_apply: Operation not permitted`。当会话本身跑在沙箱里做全量测试时，
    /// 这批"先新建沙箱再执行"的用例只能自报跳过（宿主侧仍全量执行），
    /// 否则会把系统限制误报成产品失败。
    fn skip_when_nested_sandbox() -> bool {
        if sandbox_application_refused() {
            eprintln!("skip: macOS 拒绝嵌套沙箱（本用例需要新建沙箱）");
            return true;
        }
        false
    }

    /// 沙箱里 spawn 的任何进程都不该在 Windows 上闪出控制台窗口；
    /// 统一经过 `.no_window()`，也避免回归测试逐个豁免。
    fn sandboxed_command(program: &str) -> Command {
        Command::new(program).no_window()
    }

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
        let (writable, blocked) = sandbox_paths(workspace);
        CommandSandbox::new(workspace, writable, blocked, NetworkPolicy::Enabled)
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
        if skip_when_nested_sandbox() {
            return;
        }
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
        let inside = sandboxed_command(&inside_launch.program)
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
        let denied = sandboxed_command(&outside_launch.program)
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
        if skip_when_nested_sandbox() {
            return;
        }
        let workspace = temp_dir("filemode");
        let sandbox = sandbox(&workspace);
        let profile_path = workspace.join("m224.sb");
        std::fs::write(&profile_path, sandbox.profile()).expect("write profile");

        let ok = sandboxed_command(crate::command_sandbox::MACOS_SANDBOX_EXEC)
            .args(["-f", profile_path.to_str().unwrap()])
            .args(["/bin/sh", "-c", "printf ok"])
            .current_dir(&workspace)
            .output()
            .expect("file mode spawn");
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "ok");

        let outside_file = outside_file("filemode");
        let denied = sandboxed_command(crate::command_sandbox::MACOS_SANDBOX_EXEC)
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

    fn run_git(cwd: &Path, args: &[&str]) -> std::process::Output {
        sandboxed_command("/usr/bin/git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("run git")
    }

    /// 夹具放在 HOME 下：主 checkout 与 linked worktree 都必须落在
    /// 「工作区 + 临时目录」之外，否则"主 checkout 不可写"这条断言测不到东西。
    fn fixture_dir(tag: &str) -> PathBuf {
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let dir = home.join(format!(".m224-fixture-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    /// 普通仓库 + 一个 linked worktree（`.git` 是文件，元数据在工作区之外）。
    fn linked_worktree(tag: &str) -> (PathBuf, PathBuf) {
        let base = fixture_dir(tag);
        assert!(run_git(&base, &["init", "-q", "-b", "main"]).status.success());
        assert!(
            run_git(&base, &["config", "user.email", "m224@example.com"])
                .status
                .success()
        );
        assert!(
            run_git(&base, &["config", "user.name", "m224"])
                .status
                .success()
        );
        std::fs::write(base.join("a.txt"), "a\n").expect("seed file");
        assert!(run_git(&base, &["add", "a.txt"]).status.success());
        assert!(run_git(&base, &["commit", "-qm", "init"]).status.success());
        let worktree = base.join("wt");
        assert!(
            run_git(&base, &["worktree", "add", "-q", "-b", "sandbox", "wt"])
                .status
                .success()
        );
        (base, worktree)
    }

    /// R3：真实工作区是 linked worktree 时，git 元数据（index、objects、refs、logs）
    /// 都在工作区之外。沙箱必须放行它们，否则会话里的 add/commit/merge 全部失败。
    #[test]
    fn git_metadata_outside_the_workspace_stays_writable() {
        if skip_when_nested_sandbox() {
            return;
        }
        let (base, worktree) = linked_worktree("git-worktree");
        assert!(
            worktree.join(".git").is_file(),
            "linked worktree must keep .git as a file (otherwise this test proves nothing)"
        );
        assert!(
            base.join(".git/worktrees").exists(),
            "worktree metadata must live outside the workspace"
        );

        let sandbox = sandbox(&worktree);
        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                "printf 'from sandbox\\n' > b.txt && git add b.txt \
                 && git commit -qm sandboxed && git rev-parse --short HEAD"
                    .into(),
            ],
        );
        let output = sandboxed_command(&launch.program)
            .args(&launch.args)
            .current_dir(&worktree)
            .output()
            .expect("run sandboxed git");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "sandboxed git commit must succeed inside a linked worktree: {stdout}{stderr}"
        );
        let log = run_git(&worktree, &["log", "-1", "--pretty=%s"]);
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "sandboxed",
            "the commit must be real"
        );

        let mergeable = run_git(&worktree, &["rev-parse", "--verify", "HEAD"]);
        assert!(mergeable.status.success());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 普通仓库（主 checkout 形态）：git dir 就是 common dir，index/HEAD/index.lock
    /// 都在工作区外。这里验证"整棵放行 + 后置拒绝 config/hooks"这条分支真的成立。
    #[test]
    fn git_operations_work_in_a_plain_repository_without_opening_config() {
        if skip_when_nested_sandbox() {
            return;
        }
        let base = fixture_dir("git-plain");
        assert!(run_git(&base, &["init", "-q", "-b", "main"]).status.success());
        assert!(
            run_git(&base, &["config", "user.email", "m224@example.com"])
                .status
                .success()
        );
        assert!(
            run_git(&base, &["config", "user.name", "m224"])
                .status
                .success()
        );
        std::fs::write(base.join("a.txt"), "a\n").expect("seed");
        assert!(run_git(&base, &["add", "a.txt"]).status.success());
        assert!(run_git(&base, &["commit", "-qm", "init"]).status.success());

        let config = base.join(".git/config");
        let config_before = std::fs::read_to_string(&config).expect("read config");
        let sandbox = sandbox(&base);
        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                "printf 'plain\n' > b.txt && git add b.txt && git commit -qm plain \
                 && git log -1 --pretty=%s \
                 && (printf 'x' > .git/config 2>/dev/null && echo config=WRITTEN || echo config=BLOCKED)"
                    .into(),
            ],
        );
        let output = sandboxed_command(&launch.program)
            .args(&launch.args)
            .current_dir(&base)
            .output()
            .expect("run sandboxed git");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success() && stdout.contains("plain"),
            "sandboxed commit in a plain repository must succeed: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("config=BLOCKED"),
            "git config must stay read-only even when the git dir is the workspace .git: {stdout}"
        );
        assert_eq!(
            std::fs::read_to_string(&config).expect("config unchanged"),
            config_before
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// config、hooks/ 与主 checkout 的工作树文件必须仍然不可写：
    /// 放行 git 元数据不等于允许植入"下次在沙箱外执行"的钩子或改配置。
    #[test]
    fn git_config_hooks_and_main_checkout_stay_read_only() {
        if skip_when_nested_sandbox() {
            return;
        }
        let (base, worktree) = linked_worktree("git-guard");
        let config = base.join(".git/config");
        let hook = base.join(".git/hooks/m224-probe");
        let main_file = base.join("a.txt");
        let config_before = std::fs::read_to_string(&config).expect("read config");
        let main_before = std::fs::read_to_string(&main_file).expect("read main file");

        let sandbox = sandbox(&worktree);
        let script = format!(
            "if printf 'x' > '{}' 2>/dev/null; then echo config=WRITTEN; else echo config=BLOCKED; fi; \
             if printf 'x' > '{}' 2>/dev/null; then echo hook=WRITTEN; else echo hook=BLOCKED; fi; \
             if printf 'x' > '{}' 2>/dev/null; then echo main=WRITTEN; else echo main=BLOCKED; fi",
            config.display(),
            hook.display(),
            main_file.display()
        );
        let launch = sandbox.wrap("/bin/sh", &["-c".into(), script]);
        let output = sandboxed_command(&launch.program)
            .args(&launch.args)
            .current_dir(&worktree)
            .output()
            .expect("run guard probes");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            stdout.contains("config=BLOCKED")
                && stdout.contains("hook=BLOCKED")
                && stdout.contains("main=BLOCKED"),
            "config / hooks / main checkout must stay read-only: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&config).expect("config unchanged"),
            config_before
        );
        assert!(!hook.exists(), "hook file must not be created");
        assert_eq!(
            std::fs::read_to_string(&main_file).expect("main file unchanged"),
            main_before
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 收窄 ~/.cargo：registry 与 git 可写，`bin/` 与 `config.toml` 不可写——
    /// 否则沙箱内进程能植入下次在沙箱外运行的程序。
    #[test]
    fn cargo_payload_paths_stay_read_only() {
        if skip_when_nested_sandbox() {
            return;
        }
        let workspace = temp_dir("cargo-payload");
        let sandbox = sandbox(&workspace);
        let home = PathBuf::from(std::env::var("HOME").expect("HOME"));
        let cargo_root = home.join(".cargo");
        let probe = cargo_root.join("bin/m224-probe");
        let _ = std::fs::remove_file(&probe);

        assert!(
            sandbox
                .writable()
                .iter()
                .any(|entry| entry.path().ends_with(".cargo/registry")),
            "cargo registry must stay writable: {:?}",
            sandbox.writable()
        );
        assert!(
            sandbox
                .writable()
                .iter()
                .any(|entry| entry.path().ends_with(".cargo/git")),
            "cargo git cache must stay writable"
        );
        let profile = sandbox.profile();
        assert!(
            !profile.contains(&format!(
                "(allow file-write* (subpath \"{}\"))",
                cargo_root.display()
            )),
            "the whole cargo home must never be writable: {profile}"
        );
        assert!(
            !profile.contains(".cargo/config"),
            "cargo config must never be writable: {profile}"
        );
        assert!(
            !profile.contains(".cargo/bin"),
            "cargo bin must never be writable: {profile}"
        );

        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                format!(
                    "if printf '#!/bin/sh' > '{}' 2>/dev/null; then echo bin=WRITTEN; else echo bin=BLOCKED; fi",
                    probe.display()
                ),
            ],
        );
        let output = sandboxed_command(&launch.program)
            .args(&launch.args)
            .current_dir(&workspace)
            .output()
            .expect("run cargo probe");
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            stdout.contains("bin=BLOCKED"),
            "writing into ~/.cargo/bin must be denied: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!probe.exists(), "no executable may be planted in cargo bin");
    }

    /// 供 R10 矩阵按任意工作区（例如真实 linked worktree）生成 profile。
    #[test]
    fn emits_profile_for_workspace_from_env() {
        let Ok(workspace) = std::env::var("M224_PROFILE_WORKSPACE") else {
            return;
        };
        let out = std::env::var("M224_PROFILE_OUT").expect("M224_PROFILE_OUT");
        let sandbox = sandbox(&PathBuf::from(workspace));
        std::fs::write(&out, sandbox.profile()).expect("write profile");
    }

    /// 沙箱内的信号不能越界：对沙箱外（同一用户）的进程发信号必须失败，
    /// 且那个进程要活着。这是 R3/R4 之外最容易漏的一条边界。
    #[test]
    fn signals_cannot_escape_the_sandbox() {
        if skip_when_nested_sandbox() {
            return;
        }
        let workspace = temp_dir("signal-outside");
        let sandbox = sandbox(&workspace);

        let mut outsider = sandboxed_command("/bin/sleep")
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
        let output = sandboxed_command(&launch.program)
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
        if skip_when_nested_sandbox() {
            return;
        }
        let workspace = temp_dir("signal-inside");
        let sandbox = sandbox(&workspace);
        let launch = sandbox.wrap(
            "/bin/sh",
            &[
                "-c".into(),
                "sleep 30 & child=$!; kill -TERM $child; echo inside=$?".into(),
            ],
        );
        let output = sandboxed_command(&launch.program)
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
        if skip_when_nested_sandbox() {
            return;
        }
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
        let mut child = sandboxed_command(&launch.program)
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
