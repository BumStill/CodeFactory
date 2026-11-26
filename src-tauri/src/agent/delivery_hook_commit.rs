// SPDX-License-Identifier: Apache-2.0
//! Prepare an exact child through Git's ordinary hooks, before installing it in
//! the user's checkout. The caller must journal dispatch before calling this.
use super::*;

/// One local rung only. The immutable run owner and its recovery dispatcher are
/// supplied by the host; this function never opens a PR or changes lane state.
pub(crate) async fn commit_with_hooks(
    root: &Path, scope: &TaskDeliveryScope, opts: &DeliverOpts, message: &str,
) -> Result<String, String> {
    let permit = opts.mutation_permit.as_ref().ok_or("ordinary-hook commit requires a durable mutation permit")?;
    if let Some(reason) = stop_reason(opts).await { return Err(reason); }
    permit.reacquire("git_hook_commit_prepare").await?;
    scope.verify(root)?;
    let plan = prepare_scoped_commit(root, &opts.extra_excludes)?;
    scope.verify_plan(root, &plan.target_tree_sha)?;
    let mut hook_plan = HookCommitPlan::new(root, scope, message)?;
    hook_plan.excludes = opts.extra_excludes.clone();
    let operation = external_operation_key("git_hook_commit_prepare", &[
        &scope.original_identity().repo_identity, &scope.original_identity().worktree_identity,
        &hook_plan.parent, &hook_plan.tree, message, &plan.original_index_digest, &plan.source_manifest_digest, &hook_plan.hook_policy_digest,
    ]);
    let prepared = match begin_or_reuse_external_mutation(Some(permit), "git_hook_commit_prepare", &operation,
        &serde_json::to_string(&hook_plan).map_err(|e| e.to_string())?).await? {
        DeliveryMutationBegin::AlreadyCommitted(receipt) => {
            let prepared: HookCommitPrepared = serde_json::from_value(committed_receipt_result(&receipt)?)
                .map_err(|e| e.to_string())?;
            if prepared.plan.parent != hook_plan.parent || prepared.plan.tree != hook_plan.tree
                || prepared.plan.message != message {
                return Err("recorded hook result differs from frozen preparation".into());
            }
            prepared
        },
        DeliveryMutationBegin::Dispatch(intent) => {
            let owned_root = root.to_path_buf();
            let owned_scope = scope.clone();
            let result = tokio::task::spawn_blocking(move || hook_plan.prepare(&owned_root, &owned_scope))
                .await.map_err(|e| e.to_string()).and_then(|r| r);
            let prepared = match result {
                Ok(prepared) => prepared,
                Err(error) => return Err(fail_external_mutation(Some(permit), intent.as_ref(), error).await),
            };
            commit_external_mutation(Some(permit), intent.as_ref(),
                &serde_json::to_string(&prepared).map_err(|e| e.to_string())?).await?;
            prepared
        },
    };
    if let Some(reason) = stop_reason(opts).await { return Err(reason); }
    permit.reacquire("git_local_commit").await?;
    scope.verify(root)?;
    verify_hook_policy(root, &prepared.hook_policy_digest)?;
    let mut evidence = LocalCommitIntentEvidence::prepared(
        scope.original_identity(), scope.proven_branch(), &plan.original_index_tree_sha,
        &plan.original_index_digest, &plan.target_index_digest, &plan.source_manifest_digest,
        &plan.target_tree_sha, &prepared.head, prepared.message.trim(),
    );
    evidence.hook_policy_digest = Some(prepared.hook_policy_digest.clone());
    verify_receipted_commit_object(root, scope.original_identity(), &evidence)?;
    let intent = match begin_or_reuse_external_mutation(Some(permit), "git_local_commit", &evidence.operation_key(),
        &serde_json::to_string(&evidence).map_err(|e| e.to_string())?).await? {
        DeliveryMutationBegin::Dispatch(intent) => intent,
        DeliveryMutationBegin::AlreadyCommitted(_) => return Err("local commit already settled; reconcile exact installed head before continuing".into()),
    };
    let identity = match materialize_local_commit_with_permit(Some(permit), intent.as_ref(), root, None,
        scope.proven_branch(), scope.original_identity(), &evidence).await {
        Ok(identity) => identity,
        Err(error) => return Err(fail_external_mutation(Some(permit), intent.as_ref(), error).await),
    };
    scope.verify_materialized(root)?;
    commit_external_mutation(Some(permit), intent.as_ref(), &json!({
        "head_sha":identity.head_sha, "tree_sha":plan.target_tree_sha,
        "operation_key":evidence.operation_key(),
    }).to_string()).await?;
    if let Err(error) = prepared.cleanup(root) {
        tracing::warn!(%error, "committed hook worktree retained; no forced cleanup");
    }
    Ok(identity.head_sha)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct HookCommitPlan {
    pub(crate) path: PathBuf,
    pub(crate) branch: String,
    pub(crate) parent: String,
    pub(crate) tree: String,
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) excludes: Vec<String>,
    /// Effective hooksPath and direct entry scripts; not transitive dependencies or environment.
    pub(crate) hook_policy_digest: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct HookCommitPrepared {
    pub(crate) plan: HookCommitPlan,
    pub(crate) head: String,
    pub(crate) message: String,
    /// Effective hooksPath and direct entry scripts; not transitive dependencies or environment.
    pub(crate) hook_policy_digest: String,
}

impl HookCommitPlan {
    pub(crate) fn new(root: &Path, scope: &TaskDeliveryScope, message: &str) -> Result<Self, String> {
        scope.verify(root)?;
        verify_parent_checkout_hook(root, &scope.original_identity().head_sha)?;
        let container = hook_container(root, false)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        // Keep the operation's registered checkout outside every source tree.
        // Its exact name is persisted before creation; failures retain it.
        Ok(Self {
            path: container.join(&token),
            branch: format!("codefactory/hook-commit-{token}"),
            parent: scope.original_identity().head_sha.clone(),
            tree: scope.target_tree_sha().to_string(),
            message: message.to_string(),
            excludes: Vec::new(),
            hook_policy_digest: hook_identity(root)?,
        })
    }

    pub(crate) fn prepare(&self, root: &Path, scope: &TaskDeliveryScope) -> Result<HookCommitPrepared, String> {
        scope.verify(root)?;
        if self.parent != scope.original_identity().head_sha || self.tree != scope.target_tree_sha() {
            return Err("hook preparation does not match the frozen host proof".into());
        }
        verify_parent_checkout_hook(root, &self.parent)?;
        let hooks = hook_identity(root)?;
        if hooks != self.hook_policy_digest { return Err("hook policy changed before preparation dispatch".into()); }
        let container = hook_container(root, true)?;
        if self.path.parent() != Some(container.as_path())
            || self.path.file_name().and_then(|name| name.to_str()).map(|name| format!("codefactory/hook-commit-{name}")) != Some(self.branch.clone()) {
            return Err("hook preparation path does not match its private registered branch".into());
        }
        match std::fs::symlink_metadata(&self.path) {
            Ok(_) => return Err("hook preparation checkout already exists; observe its recorded outcome, do not replay hooks".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.to_string()),
        }
        git(root, &["worktree", "add", "-b", &self.branch,
            self.path.to_str().ok_or("hook workspace path is not UTF-8")?, &self.parent])
            .map_err(|error| format!("{error}; hook preparation may have created {}; retain it for reconciliation", self.path.display()))?;
        let result = (|| {
            git(&self.path, &["read-tree", "--reset", "-u", &self.tree])?;
            if hook_identity(&self.path)? != hooks {
                return Err("effective commit hooks differ in the prepared worktree; original checkout preserved".into());
            }
            scope.verify(root)?;
            // No --no-verify, hooksPath override, commit-tree, or hook emulation.
            git(&self.path, &["commit", "-m", &self.message])?;
            let repo = git2::Repository::open(&self.path).map_err(|e| e.to_string())?;
            let commit = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| e.to_string())?;
            if commit.parent_count() != 1
                || commit.parent_id(0).map_err(|e| e.to_string())?.to_string() != self.parent
                || commit.tree_id().to_string() != self.tree
                || git(&self.path, &["branch", "--show-current"])? != self.branch
                || !git(&self.path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
                || hook_identity(&self.path)? != hooks
                || hook_identity(root)? != hooks
            {
                return Err("hooks changed the frozen tree, checkout, or hook identity; no source commit installed".into());
            }
            scope.verify(root)?;
            let message = commit.message().ok_or("hook commit message is not UTF-8")?.to_string();
            Ok(HookCommitPrepared { plan: self.clone(), head: commit.id().to_string(), message, hook_policy_digest: hooks.clone() })
        })();
        result.map_err(|error: String| format!("{error}; retained hook operation {} at {} for inspection; original HEAD/index not installed", self.branch, self.path.display()))
    }
}

impl HookCommitPrepared {
    pub(crate) fn cleanup(&self, root: &Path) -> Result<(), String> {
        let container = hook_container(root, false)?;
        if self.plan.path.parent() != Some(container.as_path())
            || self.plan.path.canonicalize().map_err(|e| e.to_string())? != self.plan.path {
            return Err("hook checkout cleanup path changed; retained without deletion".into());
        }
        if git(&self.plan.path, &["rev-parse", "HEAD"])? != self.head
            || git(&self.plan.path, &["branch", "--show-current"])? != self.plan.branch
            || !git(&self.plan.path, &["status", "--porcelain", "--untracked-files=all"])?.is_empty()
        {
            return Err("hook checkout changed after preparation; retained without forced cleanup".into());
        }
        git(root, &["worktree", "remove", self.plan.path.to_str().ok_or("hook workspace path is not UTF-8")?])?;
        git(root, &["update-ref", "-d", &format!("refs/heads/{}", self.plan.branch), &self.head])?;
        Ok(())
    }
}

pub(crate) fn verify_hook_policy(root: &Path, expected: &str) -> Result<(), String> {
    if hook_identity(root)? != expected { return Err("commit hook policy changed since successful preparation".into()); }
    Ok(())
}

fn configured_hooks_path(root: &Path) -> Result<Option<String>, String> {
    let output = dev_command("git").current_dir(root).args(["config", "--null", "--path", "--get", "core.hooksPath"])
        .output().map_err(|e| e.to_string())?;
    let configured = if output.status.success() {
        Some(String::from_utf8(output.stdout).map_err(|e| e.to_string())?
            .strip_suffix('\0').ok_or("hook configuration output lacks its NUL terminator")?.to_string())
    } else if output.status.code() == Some(1) { None }
    else { return Err("cannot resolve effective hooks configuration".into()); };
    Ok(configured)
}

fn hook_identity(root: &Path) -> Result<String, String> {
    let configured = configured_hooks_path(root)?;
    let directory = match configured.as_deref() {
        Some(value) => { let path = PathBuf::from(value); if path.is_absolute() { path } else { root.join(path) } },
        None => PathBuf::from(git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?).join("hooks"),
    };
    let mut parts = vec![configured.unwrap_or_else(|| "<default>".into())];
    for name in ["post-checkout", "pre-commit", "prepare-commit-msg", "commit-msg", "post-commit"] {
        let path = directory.join(name);
        match bounded_hook_file(&path)? {
            Some((bytes, mode)) => parts.push(format!("{name}:{mode}:{}", bytes_digest(&bytes))),
            None => parts.push(format!("{name}:absent")),
        }
    }
    Ok(external_operation_key("hook-identity", &parts.iter().map(String::as_str).collect::<Vec<_>>()))
}

fn bounded_hook_file(path: &Path) -> Result<Option<(Vec<u8>, u32)>, String> {
    use std::io::Read;
    const MAX_HOOK_BYTES: u64 = 1024 * 1024;
    if let Ok(metadata) = std::fs::metadata(path) {
        if !metadata.is_file() || metadata.len() > MAX_HOOK_BYTES {
            return Err("commit hook is not a regular file within the 1 MiB proof limit".into());
        }
    }
    let mut open = std::fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; open.custom_flags(libc::O_NONBLOCK); }
    let mut file = match open.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_HOOK_BYTES {
        return Err("commit hook is not a regular file within the 1 MiB proof limit".into());
    }
    let mut bytes = Vec::new();
    (&mut file).take(MAX_HOOK_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_HOOK_BYTES { return Err("commit hook grew beyond 1 MiB proof limit".into()); }
    #[cfg(unix)]
    let mode = { use std::os::unix::fs::PermissionsExt; metadata.permissions().mode() };
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    Ok(Some((bytes, mode)))
}

// worktree add runs post-checkout from the parent tree, before read-tree loads
// the proven candidate. A relative hooksPath therefore needs a separate proof.
fn verify_parent_checkout_hook(root: &Path, parent: &str) -> Result<(), String> {
    let Some(configured) = configured_hooks_path(root)? else { return Ok(()); };
    let directory = PathBuf::from(configured);
    if directory.is_absolute() { return Ok(()); }
    let mut relative = PathBuf::new();
    for component in directory.components() {
        match component {
            std::path::Component::Normal(name) => relative.push(name),
            std::path::Component::CurDir => {},
            _ => return Err("relative external hooksPath cannot be proven in a prepared worktree".into()),
        }
    }
    let repo = git2::Repository::open(root).map_err(|e| e.to_string())?;
    let tree = repo.find_commit(git2::Oid::from_str(parent).map_err(|e| e.to_string())?)
        .and_then(|commit| commit.tree()).map_err(|e| e.to_string())?;
    let mut prefix = PathBuf::new();
    for component in relative.components() {
        prefix.push(component);
        if let Ok(entry) = tree.get_path(&prefix) {
            if entry.filemode() != 0o040000 {
                return Err("parent hooks directory is not a plain tree; checkout hook cannot be proven".into());
            }
        }
    }
    relative.push("post-checkout");
    let expected = match tree.get_path(&relative) {
        Ok(entry) => {
            if !matches!(entry.filemode(), 0o100644 | 0o100755) {
                return Err("parent post-checkout is not a regular tracked script".into());
            }
            let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
            if blob.size() > 1024 * 1024 { return Err("parent post-checkout exceeds 1 MiB proof limit".into()); }
            Some((blob.content().to_vec(), entry.filemode()))
        },
        Err(error) if error.code() == git2::ErrorCode::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let observed = bounded_hook_file(&root.join(&relative))?;
    let matches = match (expected, observed) {
        (None, None) => true,
        (Some((before, mode)), Some((current, actual_mode))) => {
            #[cfg(unix)]
            let mode_matches = (mode == 0o100755) == (actual_mode & 0o111 != 0);
            #[cfg(not(unix))]
            let mode_matches = { let _ = (mode, actual_mode); true };
            before == current && mode_matches
        },
        _ => false,
    };
    if !matches { return Err("parent-tree post-checkout differs from the observed hook; no worktree or hook dispatched".into()); }
    Ok(())
}

fn hook_container(root: &Path, create: bool) -> Result<PathBuf, String> {
    let common = PathBuf::from(git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?).canonicalize().map_err(|e| e.to_string())?;
    let container = common.join("codefactory-hook-worktrees");
    match std::fs::symlink_metadata(&container) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("hook workspace container is not a plain owned directory".into());
        },
        Ok(_) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
            match std::fs::create_dir(&container) {
                Ok(()) => {},
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
                Err(error) => return Err(error.to_string()),
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(container),
        Err(error) => return Err(error.to_string()),
    }
    let metadata = std::fs::symlink_metadata(&container).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir()
        || container.canonicalize().map_err(|e| e.to_string())? != container {
        return Err("hook workspace container escaped its Git administrative directory".into());
    }
    Ok(container)
}
