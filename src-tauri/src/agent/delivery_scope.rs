// SPDX-License-Identifier: Apache-2.0
//! Host-derived delivery provenance. Paths and digests never come from model arguments.
use super::*;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const MAX_PATHS: usize = 20000;
const MAX_RECEIPTS: usize = 10000;
const MAX_COMMITS: usize = 256;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct TaskDeliveryScope {
    root: PathBuf,
    branch: String,
    identity: DeliveryIdentitySnapshot,
    original_index_digest: String,
    source_paths: BTreeSet<String>,
    source_digest: String,
    target_tree: String,
    requires_commit: bool,
}

fn blocked(detail: impl std::fmt::Display) -> String {
    format!("TASK_DELIVERY_SCOPE_UNPROVEN: {detail}；未证明本任务精确内容归属，保留 index、工作区及未知文件，未授权交付。")
}
fn safe_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains(['\\', '\n', '\r', '\0'])
        || path.as_bytes().get(1) == Some(&b':')
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(blocked("unsupported repository path"));
    }
    Ok(())
}
fn paths(root: &Path) -> Result<BTreeSet<String>, String> {
    let repo = git2::Repository::open(root).map_err(blocked)?;
    let mut result = BTreeSet::new();
    for entry in repo.index().map_err(blocked)?.iter() {
        let path = std::str::from_utf8(&entry.path).map_err(blocked)?;
        safe_path(path)?;
        result.insert(path.to_owned());
    }
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    for entry in repo.statuses(Some(&mut opts)).map_err(blocked)?.iter() {
        let path = entry
            .path()
            .ok_or_else(|| blocked("non-UTF8 status path"))?;
        safe_path(path)?;
        result.insert(path.to_owned());
    }
    if result.len() > MAX_PATHS {
        return Err(blocked("source path budget exceeded"));
    }
    Ok(result)
}
fn open_source_regular(path: &Path, expected: &std::fs::Metadata) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).map_err(blocked)?;
    let opened = file.metadata().map_err(blocked)?;
    if !opened.is_file() {
        return Err(blocked("source changed to a non-regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != expected.dev() || opened.ino() != expected.ino() {
            return Err(blocked("source identity changed while opening"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & 0x400 != 0
            || opened.creation_time() != expected.creation_time()
        {
            return Err(blocked("source identity changed while opening"));
        }
    }
    Ok(file)
}
fn source_digest(root: &Path, frozen: &BTreeSet<String>) -> Result<String, String> {
    let mut all = paths(root)?;
    all.extend(frozen.iter().cloned());
    if all.len() > MAX_PATHS {
        return Err(blocked("source path budget exceeded"));
    }
    let mut hasher = Sha256::new();
    let mut remaining = MAX_BYTES;
    for path in all {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        // A relative locator must not cross an ancestor symlink. Deleted paths
        // may have missing parents, but are never resolved outside this root.
        let mut ancestor = root.to_path_buf();
        if let Some(parent) = Path::new(&path).parent() {
            for component in parent.components() {
                ancestor.push(component.as_os_str());
                match std::fs::symlink_metadata(&ancestor) {
                    Ok(m) if m.file_type().is_symlink() => return Err(blocked("symlink ancestor")),
                    Ok(m) if !m.is_dir() => return Err(blocked("non-directory ancestor")),
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(blocked(e)),
                }
            }
        }
        match std::fs::symlink_metadata(root.join(&path)) {
            Ok(m) if m.file_type().is_symlink() => {
                hasher.update(b"symlink\0");
                hasher.update(
                    std::fs::read_link(root.join(&path))
                        .map_err(blocked)?
                        .as_os_str()
                        .as_encoded_bytes(),
                );
            }
            Ok(m) if m.is_file() => {
                use std::io::Read;
                if m.len() > remaining {
                    return Err(blocked("source byte budget exceeded"));
                }
                let file = open_source_regular(&root.join(&path), &m)?;
                let mut bytes = Vec::new();
                file.take(remaining + 1)
                    .read_to_end(&mut bytes)
                    .map_err(blocked)?;
                remaining = remaining
                    .checked_sub(bytes.len() as u64)
                    .ok_or_else(|| blocked("source byte budget exceeded"))?;
                hasher.update(b"file\0");
                hasher.update(bytes);
            }
            Ok(_) => return Err(blocked("unsupported source kind")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => hasher.update(b"absent\0"),
            Err(e) => return Err(blocked(e)),
        }
        hasher.update([0]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}
fn absent_digest() -> String {
    crate::agent::tool_backend::opaque_digest(&["file_content_sha256_v1", "absent"])
}
fn tree_file(
    repo: &git2::Repository,
    tree: &git2::Tree<'_>,
    path: &str,
    budget: &mut u64,
) -> Result<(String, i32), String> {
    match tree.get_path(Path::new(path)) {
        Ok(entry) => {
            if !matches!(entry.filemode(), 0o100644 | 0o100755) {
                return Err(blocked(format!("unsupported Git mode for {path}")));
            }
            let blob = repo.find_blob(entry.id()).map_err(blocked)?;
            *budget = budget
                .checked_sub(blob.size() as u64)
                .ok_or_else(|| blocked("blob byte budget exceeded"))?;
            Ok((
                format!("sha256:{:x}", Sha256::digest(blob.content())),
                entry.filemode(),
            ))
        }
        Err(e) if e.code() == git2::ErrorCode::NotFound => Ok((absent_digest(), 0)),
        Err(e) => Err(blocked(e)),
    }
}
#[derive(Debug)]
struct Edge {
    before: String,
    after: String,
}
fn prove_tree(
    repo: &git2::Repository,
    base: &git2::Tree<'_>,
    tree: &git2::Tree<'_>,
    edges: &BTreeMap<String, Vec<Edge>>,
    budget: &mut u64,
) -> Result<(), String> {
    let diff = repo
        .diff_tree_to_tree(Some(base), Some(tree), None)
        .map_err(blocked)?;
    if diff.deltas().len() > MAX_PATHS {
        return Err(blocked("changed path budget exceeded"));
    }
    for delta in diff.deltas() {
        for file in [delta.old_file(), delta.new_file()] {
            let Some(path) = file.path() else { continue };
            let path = path
                .to_str()
                .ok_or_else(|| blocked("non-UTF8 changed path"))?;
            safe_path(path)?;
            let (before, old_mode) = tree_file(repo, base, path, budget)?;
            let (after, new_mode) = tree_file(repo, tree, path, budget)?;
            if before == after && old_mode == new_mode {
                continue;
            }
            if old_mode != 0 && new_mode != 0 && old_mode != new_mode {
                return Err(blocked(format!("unreceipted mode change: {path}")));
            }
            if old_mode == 0 && new_mode != 0o100644 && new_mode != 0 {
                return Err(blocked(format!("unreceipted executable creation: {path}")));
            }
            // Millisecond timestamps are not a total dispatch order. Resolve
            // only edges reachable from the trusted base digest; a bounded
            // graph handles equal timestamps without trusting receipt IDs.
            let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
            for edge in edges.get(path).into_iter().flatten() {
                adjacency.entry(&edge.before).or_default().push(&edge.after);
            }
            let mut reachable = BTreeSet::from([before.clone()]);
            let mut pending = vec![before];
            while let Some(from) = pending.pop() {
                for next in adjacency.get(from.as_str()).into_iter().flatten() {
                    if reachable.insert((*next).to_string()) {
                        pending.push((*next).to_string());
                    }
                }
            }
            if !reachable.contains(&after) {
                return Err(blocked(format!("missing exact receipt chain: {path}")));
            }
        }
    }
    Ok(())
}
impl TaskDeliveryScope {
    pub(crate) async fn load(
        db: &sqlx::SqlitePool,
        objective_id: &str,
        root: &Path,
        excludes: &[String],
    ) -> Result<Self, String> {
        let workspace =
            crate::agent::execution_workspace::verify_objective_workspace(db, objective_id, root)
                .await
                .map_err(blocked)?;
        let root = root.canonicalize().map_err(blocked)?;
        let context = resolve_repo(&root, None)?;
        let identity = capture_delivery_identity(&context)?;
        let original_index_digest = super::current_index_digest(&root)?;
        let source_paths = paths(&root)?;
        let source_digest = source_digest(&root, &source_paths)?;
        let plan = super::prepare_scoped_commit(&root, excludes)?;
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM objectives WHERE id=? AND status IN ('active','waiting_system')",
        )
        .bind(objective_id)
        .fetch_one(db)
        .await
        .map_err(blocked)?;
        // A receipt is authoritative only if its host-recorded arguments bind
        // this exact working directory and current binding generation. Merely
        // sharing an Objective or observing a changed file is not authorship.
        let rows=sqlx::query("SELECT c.safe_locator_json,c.precondition_digest,c.expected_postcondition_digest,
            c.observed_digest,c.action_fingerprint,r.revision,b.id AS binding_id,b.resource_generation,
            t.tool_name,t.arguments FROM side_effect_observation_contracts c
            JOIN side_effect_receipts r ON r.id=c.receipt_id AND r.objective_id=c.objective_id AND r.binding_id=c.binding_id
            JOIN objective_bindings b ON b.id=c.binding_id AND b.objective_id=c.objective_id
            JOIN tool_recovery_call_links l ON l.receipt_id=r.id
            JOIN tool_calls t ON t.id=l.tool_call_id
            WHERE c.objective_id=? AND c.state='applied' AND c.observer_kind='file_content_sha256_v1'
            AND c.operation_domain='tool_file' AND r.status IN ('committed','reconciled')
            AND r.action_fingerprint=c.action_fingerprint AND t.status='done'
            AND NOT EXISTS(SELECT 1 FROM objective_bindings newer WHERE newer.domain=b.domain AND newer.resource_kind=b.resource_kind AND newer.resource_id=b.resource_id
                AND newer.resource_generation>b.resource_generation)
            ORDER BY c.created_at,c.observed_at,c.receipt_id LIMIT 10001")
            .bind(objective_id).fetch_all(db).await.map_err(blocked)?;
        if rows.len() > MAX_RECEIPTS {
            return Err(blocked("receipt budget exceeded"));
        }
        let mut groups: Vec<Vec<(String, Edge)>> = Vec::new();
        for row in rows {
            let rev: i64 = row.get("revision");
            if rev < 1 || rev > revision {
                continue;
            }
            let expected: String = row.get("expected_postcondition_digest");
            if row.get::<Option<String>, _>("observed_digest").as_deref() != Some(expected.as_str())
            {
                continue;
            }
            let name: String = row.get("tool_name");
            let arguments: String = row.get("arguments");
            let Ok(args) = serde_json::from_str::<serde_json::Value>(&arguments) else {
                continue;
            };
            let binding: String = row.get("binding_id");
            let generation: i64 = row.get("resource_generation");
            let fingerprint = crate::agent::tool_backend::opaque_digest(&[
                &name,
                &crate::agent::tool_backend::canonical_json(&args),
                root.to_str().ok_or_else(|| blocked("non-UTF8 root"))?,
                &binding,
                &generation.to_string(),
            ]);
            if fingerprint != row.get::<String, _>("action_fingerprint") {
                continue;
            }
            let locator: String = row.get("safe_locator_json");
            let value: serde_json::Value = serde_json::from_str(&locator).map_err(blocked)?;
            if let Some(manifest) = value.get("shell_intent_v1") {
                let manifest: crate::agent::tool_backend::shell_intent::Manifest =
                    serde_json::from_value(manifest.clone()).map_err(blocked)?;
                if !manifest.valid()
                    || manifest.digest(false) != row.get::<String, _>("precondition_digest")
                    || manifest.digest(true) != expected
                {
                    return Err(blocked("shell manifest aggregate mismatch"));
                }
                let mut group = Vec::new();
                for entry in manifest.entries {
                    safe_path(&entry.path)?;
                    group.push((
                        entry.path,
                        Edge {
                            before: entry.before,
                            after: entry.after,
                        },
                    ));
                }
                groups.push(group);
            } else {
                let path = value
                    .get("workspace_relative_path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| blocked("receipt path missing"))?;
                safe_path(path)?;
                groups.push(vec![(
                    path.into(),
                    Edge {
                        before: row.get("precondition_digest"),
                        after: expected,
                    },
                )]);
            }
        }
        // Thin (and actual foreground legacy) dispatches record content,
        // not a second business/recovery state machine. Historical completed
        // observations stay available across later permission-mode changes.
        for manifest in crate::agent::tool_backend::content_provenance::completed(db, &workspace)
            .await
            .map_err(blocked)?
        {
            groups.push(
                manifest
                    .entries
                    .into_iter()
                    .map(|entry| {
                        (
                            entry.path,
                            Edge {
                                before: entry.before,
                                after: entry.after,
                            },
                        )
                    })
                    .collect(),
            );
        }
        let repo = git2::Repository::open(&root).map_err(blocked)?;
        let base_oid = git2::Oid::from_str(&workspace.base_sha).map_err(blocked)?;
        let head = git2::Oid::from_str(&identity.head_sha).map_err(blocked)?;
        if head != base_oid && !repo.graph_descendant_of(head, base_oid).map_err(blocked)? {
            return Err(blocked("workspace baseline is not an ancestor"));
        }
        let base = repo
            .find_commit(base_oid)
            .map_err(blocked)?
            .tree()
            .map_err(blocked)?;
        let mut budget = MAX_BYTES;
        // Whole intent groups require every input, including unchanged reads,
        // to connect to the baseline before ANY output edge is admitted.
        // A foreign source cannot bless its copied output through a self-loop.
        let mut reachable: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        if groups.iter().map(Vec::len).sum::<usize>() > MAX_RECEIPTS {
            return Err(blocked("receipt member budget exceeded"));
        }
        for group in &groups {
            for (path, _) in group {
                if !reachable.contains_key(path) {
                    let (digest, _) = tree_file(&repo, &base, path, &mut budget)?;
                    reachable.insert(path.clone(), BTreeSet::from([digest]));
                }
            }
        }
        let mut edges: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        let mut work = 0usize;
        loop {
            let mut progressed = false;
            let mut index = 0;
            while index < groups.len() {
                work += groups[index].len();
                if work > 1_000_000 {
                    return Err(blocked("receipt dependency budget exceeded"));
                }
                if groups[index].iter().all(|(path, edge)| {
                    reachable
                        .get(path)
                        .is_some_and(|set| set.contains(&edge.before))
                }) {
                    let group = groups.swap_remove(index);
                    for (path, edge) in group {
                        reachable
                            .get_mut(&path)
                            .expect("initialized dependency")
                            .insert(edge.after.clone());
                        edges.entry(path).or_default().push(edge);
                    }
                    progressed = true;
                } else {
                    index += 1;
                }
            }
            if !progressed || groups.is_empty() {
                break;
            }
        }
        let mut walk = repo.revwalk().map_err(blocked)?;
        walk.push(head).map_err(blocked)?;
        walk.hide(base_oid).map_err(blocked)?;
        for (n, oid) in walk.enumerate() {
            if n >= MAX_COMMITS {
                return Err(blocked("commit history budget exceeded"));
            }
            let tree = repo
                .find_commit(oid.map_err(blocked)?)
                .map_err(blocked)?
                .tree()
                .map_err(blocked)?;
            prove_tree(&repo, &base, &tree, &edges, &mut budget)?;
        }
        for sha in [&plan.original_index_tree_sha, &plan.target_tree_sha] {
            let tree = repo
                .find_tree(git2::Oid::from_str(sha).map_err(blocked)?)
                .map_err(blocked)?;
            prove_tree(&repo, &base, &tree, &edges, &mut budget)?;
        }
        let scope = Self {
            root,
            branch: context.branch,
            identity,
            original_index_digest,
            source_paths,
            source_digest,
            requires_commit: repo
                .find_commit(head)
                .map_err(blocked)?
                .tree_id()
                .to_string()
                != plan.target_tree_sha,
            target_tree: plan.target_tree_sha,
        };
        scope.verify(&scope.root)?;
        Ok(scope)
    }
    fn verify_source(&self, root: &Path) -> Result<(), String> {
        if root.canonicalize().map_err(blocked)? != self.root
            || source_digest(root, &self.source_paths)? != self.source_digest
        {
            return Err(blocked("source changed after task provenance verification"));
        }
        let context = resolve_repo(root, None)?;
        if context.branch != self.branch {
            return Err(blocked("branch changed after task provenance verification"));
        }
        let current = capture_delivery_identity(&context)?;
        if current.repo_identity != self.identity.repo_identity
            || current.worktree_identity != self.identity.worktree_identity
        {
            return Err(blocked("workspace identity changed"));
        }
        Ok(())
    }
    pub(crate) fn target_tree_sha(&self) -> &str {
        &self.target_tree
    }
    pub(crate) fn proven_branch(&self) -> &str {
        &self.branch
    }
    pub(crate) fn original_identity(&self) -> &DeliveryIdentitySnapshot {
        &self.identity
    }
    pub(crate) fn committed_head_sha(&self) -> &str {
        &self.identity.head_sha
    }
    pub(crate) fn proven_push_refspec(&self) -> String {
        format!("{}:refs/heads/{}", self.identity.head_sha, self.branch)
    }
    pub(crate) fn requires_commit(&self) -> bool {
        self.requires_commit
    }
    pub(crate) fn verify(&self, root: &Path) -> Result<(), String> {
        self.verify_source(root)?;
        if git(root, &["rev-parse", "HEAD"])? != self.identity.head_sha
            || super::current_index_digest(root)? != self.original_index_digest
        {
            return Err(blocked(
                "HEAD or index changed after task provenance verification",
            ));
        }
        Ok(())
    }
    pub(crate) fn verify_materialized(&self, root: &Path) -> Result<(), String> {
        self.verify_source(root)?;
        let repo = git2::Repository::open(root).map_err(blocked)?;
        if repo
            .index()
            .map_err(blocked)?
            .write_tree()
            .map_err(blocked)?
            .to_string()
            != self.target_tree
        {
            return Err(blocked("index differs from proven target tree"));
        }
        let head = repo
            .head()
            .map_err(blocked)?
            .peel_to_commit()
            .map_err(blocked)?;
        if head.id().to_string() != self.identity.head_sha
            && (head.parent_count() != 1
                || head.parent_id(0).map_err(blocked)?.to_string() != self.identity.head_sha
                || head.tree_id().to_string() != self.target_tree)
        {
            return Err(blocked("HEAD is not the exact proven child commit"));
        }
        Ok(())
    }
    pub(crate) fn verify_plan(&self, root: &Path, target: &str) -> Result<(), String> {
        self.verify(root)?;
        if target != self.target_tree {
            return Err(blocked("planned target differs from verified task content"));
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod source_open_tests {
    use super::*;
    #[test]
    fn r2_scope_source_open_rejects_fifo_and_link_replacement_without_blocking() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let raw = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let opened_path = path.clone();
        let expected = metadata.clone();
        std::thread::spawn(move || {
            tx.send(open_source_regular(&opened_path, &expected).is_err())
                .unwrap();
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("FIFO open must not block"));
        std::fs::remove_file(&path).unwrap();
        let target = temp.path().join("foreign.txt");
        std::fs::write(&target, b"foreign").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(open_source_regular(&path, &metadata).is_err());
    }
}
