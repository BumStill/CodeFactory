// SPDX-License-Identifier: Apache-2.0
//! Exact-executable DeliveryRun recovery smoke.
//!
//! The parent creates a real Git worktree, bare remote, and production SQLite
//! database. It hard-kills one copy after the exact commit object and durable
//! write-ahead intent exist but before the branch ref CAS. A replacement must
//! materialize and reconcile that exact child once under a fresh owner fence.
//! The same replacement then creates an unreceipted foreign commit and proves
//! repeated takeover remains mutation-free and parks at a bounded ceiling.

use crate::util::no_window::NoWindow;
use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn git(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn seed_git_fixture(root: &Path) -> anyhow::Result<()> {
    let origin = root.join("origin.git");
    let worktree = root.join("worktree");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q"])?;
    std::fs::create_dir_all(&worktree)?;
    git(&worktree, &["init", "-q"])?;
    git(&worktree, &["config", "user.name", "Delivery Smoke"])?;
    git(
        &worktree,
        &["config", "user.email", "delivery-smoke@example.invalid"],
    )?;
    std::fs::write(worktree.join("README.md"), "# delivery recovery smoke\n")?;
    git(&worktree, &["add", "README.md"])?;
    git(&worktree, &["commit", "-q", "-m", "chore: seed fixture"])?;
    git(&worktree, &["branch", "-M", "main"])?;
    git(
        &worktree,
        &["remote", "add", "origin", origin.to_string_lossy().as_ref()],
    )?;
    git(&worktree, &["push", "-q", "-u", "origin", "main"])?;
    git(
        &worktree,
        &["checkout", "-q", "-b", "fix/delivery-recovery-smoke"],
    )?;
    git(
        &worktree,
        &["push", "-q", "-u", "origin", "fix/delivery-recovery-smoke"],
    )?;
    std::fs::write(
        worktree.join("recovery.txt"),
        "resume this exact receipted commit\n",
    )?;
    Ok(())
}

fn spawn_worker(state_dir: &Path, phase: &str) -> anyhow::Result<std::process::Child> {
    Command::new(std::env::current_exe()?)
        .no_window()
        .arg("--delivery-recovery-worker")
        .arg(state_dir)
        .arg(phase)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawn delivery recovery worker")
}

async fn wait_for_marker(
    child: &mut std::process::Child,
    marker: &Path,
    phase: &str,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        if marker.exists() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("delivery recovery {phase} worker exited before marker: {status}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("delivery recovery {phase} worker did not reach its marker within 45 seconds")
}

pub(crate) async fn run_parent() -> anyhow::Result<serde_json::Value> {
    let smoke_id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("codefactory-delivery-recovery-{smoke_id}"));
    std::fs::create_dir_all(&root)?;
    seed_git_fixture(&root)?;

    let result = async {
        let seed_marker = root.join("seed-ready.json");
        let mut seed = spawn_worker(&root, "seed")?;
        wait_for_marker(&mut seed, &seed_marker, "seed").await?;
        let seed_receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&seed_marker)?)?;
        let seed_pid = seed_receipt
            .get("worker_pid")
            .and_then(serde_json::Value::as_u64)
            .context("seed marker omitted worker_pid")?;
        let observed_pre_ref_head = git(&root.join("worktree"), &["rev-parse", "HEAD"])?;
        let previous_head = seed_receipt
            .get("previous_head_sha")
            .and_then(serde_json::Value::as_str)
            .context("seed marker omitted previous_head_sha")?;
        let expected_head = seed_receipt
            .get("expected_head_sha")
            .and_then(serde_json::Value::as_str)
            .context("seed marker omitted expected_head_sha")?;
        let original_index_digest = seed_receipt
            .get("original_index_digest")
            .and_then(serde_json::Value::as_str)
            .context("seed marker omitted original_index_digest")?;
        let target_index_digest = seed_receipt
            .get("target_index_digest")
            .and_then(serde_json::Value::as_str)
            .context("seed marker omitted target_index_digest")?;
        let repository = git2::Repository::open(root.join("worktree"))?;
        let index_path = repository
            .index()?
            .path()
            .context("delivery smoke repository omitted index path")?
            .to_path_buf();
        let index_lock_path = index_path.with_extension("lock");
        let observed_index_digest = format!(
            "sha256:{:x}",
            Sha256::digest(std::fs::read(&index_path)?)
        );
        let observed_index_lock_digest = format!(
            "sha256:{:x}",
            Sha256::digest(std::fs::read(&index_lock_path)?)
        );
        let owned_lock_path = seed_receipt
            .get("owned_lock_path")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from)
            .context("seed marker omitted owned_lock_path")?;
        let owned_lock_matches_standard = same_file::is_same_file(&owned_lock_path, &index_lock_path)?;
        let pre_ref = seed_receipt
            .get("post_intent_target_index_lock_pre_ref_fault_injected")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        if !pre_ref
            || observed_pre_ref_head != previous_head
            || observed_pre_ref_head == expected_head
            || observed_index_digest != original_index_digest
            || observed_index_lock_digest != target_index_digest
            || !owned_lock_matches_standard
        {
            bail!(
                "delivery recovery seed did not stop with intent+exact target index lock durable before the branch ref CAS"
            );
        }
        seed.kill()
            .context("hard-kill delivery recovery seed worker")?;
        let killed_status = seed.wait().context("reap delivery recovery seed worker")?;
        if killed_status.success() {
            bail!("delivery recovery seed worker was not hard-killed");
        }
        let db_url = format!("sqlite:{}", root.join("delivery-recovery.db").display());
        let pool = crate::storage::db::connect(&db_url).await?;
        let (intent_status, intent_evidence): (String, String) = sqlx::query_as(
            "SELECT status, evidence_json FROM delivery_mutation_intents
             WHERE run_id='delivery-recovery-smoke-receipted'
               AND rung='git_local_commit'",
        )
        .fetch_one(&pool)
        .await?;
        let intent_evidence: serde_json::Value = serde_json::from_str(&intent_evidence)?;
        if intent_status != "started"
            || intent_evidence
                .get("previous_head_sha")
                .and_then(serde_json::Value::as_str)
                != Some(previous_head)
            || intent_evidence
                .get("expected_head_sha")
                .and_then(serde_json::Value::as_str)
                != Some(expected_head)
            || intent_evidence
                .get("staged_tree_sha")
                .and_then(serde_json::Value::as_str)
                .is_none()
            || intent_evidence
                .get("original_index_digest")
                .and_then(serde_json::Value::as_str)
                != Some(original_index_digest)
            || intent_evidence
                .get("target_index_digest")
                .and_then(serde_json::Value::as_str)
                != Some(target_index_digest)
        {
            bail!("delivery recovery seed marker did not match the durable exact intent");
        }
        crate::storage::db::close_and_release_files(pool).await;

        let rebind_marker = root.join("rebind-ready.json");
        let mut rebind = spawn_worker(&root, "rebind")?;
        wait_for_marker(&mut rebind, &rebind_marker, "rebind").await?;
        let rebind_receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&rebind_marker)?)?;
        let rebind_pid = rebind_receipt
            .get("worker_pid")
            .and_then(serde_json::Value::as_u64)
            .context("rebind marker omitted worker_pid")?;
        rebind
            .kill()
            .context("hard-kill delivery identity-rebind worker")?;
        let rebind_status = rebind
            .wait()
            .context("reap delivery identity-rebind worker")?;
        if rebind_status.success()
            || rebind_pid == seed_pid
            || rebind_receipt
                .get("identity_revision_count")
                .and_then(serde_json::Value::as_i64)
                != Some(1)
            || rebind_receipt
                .get("canonical_parent_mutation_count")
                .and_then(serde_json::Value::as_i64)
                != Some(0)
        {
            bail!("delivery identity rebind did not reach the second process-loss boundary safely");
        }

        let push_marker = root.join("push-ready.json");
        let mut push = spawn_worker(&root, "push")?;
        wait_for_marker(&mut push, &push_marker, "push").await?;
        let push_receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&push_marker)?)?;
        let push_pid = push_receipt
            .get("worker_pid")
            .and_then(serde_json::Value::as_u64)
            .context("push marker omitted worker_pid")?;
        let db_url = format!("sqlite:{}", root.join("delivery-recovery.db").display());
        let pool = crate::storage::db::connect(&db_url).await?;
        let (push_intent_count, delivery_status): (i64, String) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM delivery_mutation_intents
                     WHERE run_id=delivery_runs.id AND rung='git_push' AND status='committed'),
                    status
             FROM delivery_runs WHERE id='delivery-recovery-smoke-receipted'",
        )
        .fetch_one(&pool)
        .await?;
        crate::storage::db::close_and_release_files(pool).await;
        let pushed_remote_head = git(
            &root.join("origin.git"),
            &["rev-parse", "refs/heads/fix/delivery-recovery-smoke"],
        )?;
        if push_pid == seed_pid
            || push_pid == rebind_pid
            || push_receipt
                .get("post_remote_commit_pre_outcome_fault_injected")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || push_receipt.get("rung").and_then(serde_json::Value::as_str)
                != Some("git_push")
            || push_intent_count != 1
            || pushed_remote_head != expected_head
            || matches!(delivery_status.as_str(), "awaiting_completion_arbitration" | "completed")
        {
            bail!(
                "delivery push did not stop after one committed remote receipt and before durable outcome persistence"
            );
        }
        push.kill()
            .context("hard-kill post-push delivery worker")?;
        let push_status = push.wait().context("reap post-push delivery worker")?;
        if push_status.success() {
            bail!("post-push delivery worker was not hard-killed");
        }

        let recover_marker = root.join("recover-result.json");
        let mut recover = spawn_worker(&root, "recover")?;
        wait_for_marker(&mut recover, &recover_marker, "recover").await?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let recover_status = loop {
            if let Some(status) = recover.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = recover.kill();
                let _ = recover.wait();
                bail!("delivery recovery replacement worker did not exit");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        if !recover_status.success() {
            bail!("delivery recovery replacement worker exited {recover_status}");
        }
        let recovered: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&recover_marker)?)?;
        let recovery_pid = recovered
            .get("worker_pid")
            .and_then(serde_json::Value::as_u64)
            .context("recovery marker omitted worker_pid")?;
        if seed_pid == recovery_pid || rebind_pid == recovery_pid || push_pid == recovery_pid {
            bail!("delivery recovery smoke did not cross four real process owners");
        }
        if recovered
            .get("exact_receipted_head_reconciled")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
            || recovered
                .get("canonical_parent_reconciled")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("canonical_parent_mutation_count")
                .and_then(serde_json::Value::as_i64)
                != Some(0)
            || recovered
                .get("foreign_identity_parked")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("claim_epoch_plateau")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("duplicate_remote_write_count")
                .and_then(serde_json::Value::as_i64)
                != Some(0)
            || recovered
                .get("production_resume_path")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("completion_arbiter_converged")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("single_push_receipt_count")
                .and_then(serde_json::Value::as_i64)
                != Some(1)
            || recovered
                .get("canonical_pr_number")
                .and_then(serde_json::Value::as_i64)
                != Some(1)
            || recovered
                .get("recovery_parked_event_count")
                .and_then(serde_json::Value::as_i64)
                != Some(1)
            || recovered
                .get("remote_head_unchanged")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || recovered
                .get("user_message_count")
                .and_then(serde_json::Value::as_i64)
                != Some(1)
            || recovered
                .get("human_prompt_count")
                .and_then(serde_json::Value::as_i64)
                != Some(0)
        {
            bail!(
                "delivery recovery smoke receipt rejected the recovered trajectory: {}",
                serde_json::to_string(&recovered)?
            );
        }
        Ok(serde_json::json!({
            "ok": true,
            "scenario_id": "E2E-011",
            "scenario_ids": ["HLT-001", "HLT-002", "HLT-005", "CXD-002", "E2E-011"],
            "build_git_sha": option_env!("CODEFACTORY_BUILD_GIT_SHA").unwrap_or("unknown"),
            "process_restart_observed": true,
            "post_commit_owner_hard_killed": true,
            "pre_ref_owner_hard_killed": true,
            "post_rebind_owner_hard_killed": true,
            "post_push_owner_hard_killed": true,
            "post_push_pre_outcome_receipt_reconciled": true,
            "four_process_owners_observed": true,
            "same_delivery_run": true,
            "exact_receipted_head_reconciled": recovered["exact_receipted_head_reconciled"],
            "identity_revision_count": recovered["identity_revision_count"],
            "canonical_parent_reconciled": recovered["canonical_parent_reconciled"],
            "canonical_parent_mutation_count": recovered["canonical_parent_mutation_count"],
            "foreign_identity_parked": recovered["foreign_identity_parked"],
            "claim_epoch_plateau": recovered["claim_epoch_plateau"],
            "claim_epoch": recovered["claim_epoch"],
            "recovery_parked_event_count": recovered["recovery_parked_event_count"],
            "duplicate_remote_write_count": recovered["duplicate_remote_write_count"],
            "production_resume_path": recovered["production_resume_path"],
            "completion_arbiter_converged": recovered["completion_arbiter_converged"],
            "single_push_receipt_count": recovered["single_push_receipt_count"],
            "canonical_pr_number": recovered["canonical_pr_number"],
            "remote_head_unchanged": recovered["remote_head_unchanged"],
            "user_message_count": recovered["user_message_count"],
            "human_prompt_count": recovered["human_prompt_count"],
            "cleanup_ok": false,
        }))
    }
    .await;

    crate::util::fs_cleanup::remove_fixture_dir(&root).await;
    let cleanup_ok = !root.exists();
    match result {
        Ok(mut receipt) if cleanup_ok => {
            receipt["cleanup_ok"] = serde_json::Value::Bool(true);
            Ok(receipt)
        }
        Ok(_) => bail!("delivery recovery smoke leaked isolated state"),
        Err(error) => Err(error),
    }
}

pub(crate) async fn run_worker(state_dir: &Path, phase: &str) -> anyhow::Result<()> {
    match phase {
        "seed" => {
            let mut marker =
                crate::tools::delivery::seed_delivery_recovery_smoke(state_dir).await?;
            marker["worker_pid"] = serde_json::Value::from(std::process::id());
            std::fs::write(
                state_dir.join("seed-ready.json"),
                serde_json::to_vec_pretty(&marker)?,
            )?;
            tokio::time::sleep(Duration::from_secs(300)).await;
            bail!("delivery recovery seed worker was not killed at the injected fault point")
        }
        "rebind" => {
            crate::tools::delivery::rebind_delivery_recovery_smoke(state_dir).await?;
            bail!("delivery recovery rebind worker returned before it was hard-killed")
        }
        "push" => {
            crate::tools::delivery::push_delivery_recovery_smoke(state_dir).await?;
            bail!("delivery recovery push worker returned before it was hard-killed")
        }
        "recover" => {
            let mut marker =
                crate::tools::delivery::recover_delivery_recovery_smoke(state_dir).await?;
            marker["worker_pid"] = serde_json::Value::from(std::process::id());
            std::fs::write(
                state_dir.join("recover-result.json"),
                serde_json::to_vec_pretty(&marker)?,
            )?;
            Ok(())
        }
        _ => bail!("unknown delivery recovery worker phase {phase}"),
    }
}
