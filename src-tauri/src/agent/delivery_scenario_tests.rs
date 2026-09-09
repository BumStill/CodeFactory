// SPDX-License-Identifier: Apache-2.0
//! Synthetic backend prerequisite for CF-STG-R14/R29, not desktop E2E evidence.

use super::*;
use crate::util::process_tree::{isolate_std_process_tree, StdProcessTree};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

const WORKER: &str = "agent::delivery::scenario_tests::delivery_fake_forge_worker";
const DESCENDANT: &str = "agent::delivery::scenario_tests::owned_survivor_descendant";
const DIRTY: &str = "synthetic user edit: must remain uncommitted\n";

#[cfg(target_os = "macos")]
fn reserved_leader_is_only_member(
    count: i32,
    buffer: &[i32],
    leader: u32,
) -> std::io::Result<bool> {
    // proc_listpgrppids returns a PID count, unlike proc_listpids' byte count.
    // Apple libproc.c: proc_listpids(...) / sizeof(int), verified independently:
    // https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c#L70-L78
    // Reject absent leader, empty/unknown/truncated results and invalid entries.
    if count <= 0 || count as usize >= buffer.len() {
        return Err(std::io::Error::other(
            "owned process group result is empty, failed or truncated",
        ));
    }
    let members = &buffer[..count as usize];
    if members.iter().any(|pid| *pid <= 0)
        || !members.contains(&(leader as i32))
        || members.iter().copied().collect::<BTreeSet<_>>().len() != members.len()
    {
        return Err(std::io::Error::other(
            "owned process group lacks exact reserved leader or valid members",
        ));
    }
    Ok(members.len() == 1)
}

#[test]
#[cfg(target_os = "macos")]
fn owned_group_observation_rejects_empty_missing_leader_and_truncation() {
    assert!(reserved_leader_is_only_member(1, &[41, 0, 0], 41).unwrap());
    assert!(!reserved_leader_is_only_member(2, &[41, 42, 0], 41).unwrap());
    for (count, members) in [
        (-1, vec![41, 0]),
        (0, vec![0, 0]),
        (1, vec![0, 0]),
        (1, vec![42, 0]),
        (2, vec![41, 42]),
        (3, vec![41, 0]),
        (2, vec![41, 41, 0]),
        (4, vec![41, 0, 0, 0, 0]),
    ] {
        assert!(reserved_leader_is_only_member(count, &members, 41).is_err());
    }
}

struct FakeForge {
    db: sqlx::SqlitePool,
    world: PathBuf,
}

impl FakeForge {
    async fn open(world: &Path) -> Self {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(world.join("forge.sqlite"))
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS mutations (identity TEXT PRIMARY KEY, payload TEXT NOT NULL)")
            .execute(&db).await.unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS requests (kind TEXT NOT NULL, identity TEXT NOT NULL)",
        )
        .execute(&db)
        .await
        .unwrap();
        Self {
            db,
            world: world.to_path_buf(),
        }
    }

    async fn record(&self, identity: &str, payload: &serde_json::Value) {
        sqlx::query("INSERT OR IGNORE INTO mutations(identity, payload) VALUES (?, ?)")
            .bind(identity)
            .bind(payload.to_string())
            .execute(&self.db)
            .await
            .unwrap();
        assert_eq!(
            self.observed(identity).await.as_ref(),
            Some(payload),
            "same forge identity must not silently acquire a different mutation"
        );
    }

    async fn observed(&self, identity: &str) -> Option<serde_json::Value> {
        sqlx::query_scalar::<_, String>("SELECT payload FROM mutations WHERE identity = ?")
            .bind(identity)
            .fetch_optional(&self.db)
            .await
            .unwrap()
            .map(|raw| serde_json::from_str(&raw).unwrap())
    }

    async fn request(&self, kind: &str, identity: &str) {
        sqlx::query("INSERT INTO requests(kind, identity) VALUES (?, ?)")
            .bind(kind)
            .bind(identity)
            .execute(&self.db)
            .await
            .unwrap();
    }

    async fn counts(&self) -> (usize, usize, usize) {
        // Recompute from persisted raw rows, never from adapter-supplied counters.
        let rows = sqlx::query_scalar::<_, String>("SELECT payload FROM mutations")
            .fetch_all(&self.db)
            .await
            .unwrap();
        let mut counts = (0, 0, 0);
        for raw in rows {
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            match value["kind"].as_str() {
                Some("pr") => counts.0 += 1,
                Some("merge") => counts.1 += 1,
                Some("release") => counts.2 += 1,
                _ => panic!("unknown persistent mutation"),
            }
        }
        counts
    }

    async fn assert_single_dispatches(&self) {
        for kind in ["pr", "merge", "release"] {
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests WHERE kind = ?")
                .bind(kind)
                .fetch_one(&self.db)
                .await
                .unwrap();
            assert_eq!(
                count, 1,
                "production must not redispatch {kind} after receipt recovery"
            );
        }
        assert_eq!(self.counts().await, (1, 1, 1));
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.db)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    fn origin(&self) -> PathBuf {
        self.world.join("origin.git")
    }

    fn pr(value: &serde_json::Value) -> DeliveryPr {
        DeliveryPr {
            number: value["number"].as_u64().unwrap(),
            url: value["url"].as_str().unwrap().into(),
            title: value["title"].as_str().unwrap().into(),
            body: value["body"].as_str().unwrap().into(),
        }
    }
}

impl DeliveryRemote for FakeForge {
    fn capabilities(&self) -> DeliveryCapabilities {
        DeliveryCapabilities {
            review: true,
            ci: true,
            merge: true,
            release: true,
            live: true,
        }
    }

    async fn open_or_get_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        expected_head_sha: &str,
        permit: Option<&DeliveryMutationPermit>,
    ) -> Result<DeliveryPr, String> {
        assert!(
            permit.is_none(),
            "this slice does not exercise durable DeliveryRun permits"
        );
        let identity = format!("pr:{head}:{base}");
        self.request("pr_lookup", &identity).await;
        if let Some(value) = self.observed(&identity).await {
            assert_eq!(value["head_sha"], expected_head_sha);
            return Ok(Self::pr(&value));
        }
        assert_eq!(
            git(
                &self.origin(),
                &["rev-parse", &format!("refs/heads/{head}")]
            )?,
            expected_head_sha
        );
        self.request("pr", &identity).await;
        let value = json!({"kind": "pr", "number": 1, "url": "https://forge.invalid/pr/1",
            "title": title, "body": body, "head": head, "base": base, "head_sha": expected_head_sha});
        self.record(&identity, &value).await;
        Ok(Self::pr(&value))
    }

    async fn observe_open_pr(&self, head: &str, base: &str) -> Result<OpenPrObservation, String> {
        Ok(match self.observed(&format!("pr:{head}:{base}")).await {
            Some(value) => OpenPrObservation::Open(OpenPrState {
                pr: Self::pr(&value),
                head_branch: head.into(),
                base_branch: base.into(),
                head_sha: Some(value["head_sha"].as_str().unwrap().into()),
            }),
            None => OpenPrObservation::Absent,
        })
    }

    async fn ci_status(&self, sha: &str) -> Result<CiStatus, String> {
        self.request("ci", sha).await;
        assert_eq!(
            git(&self.origin(), &["rev-parse", "refs/heads/feat/synthetic"])?,
            sha
        );
        Ok(
            match std::fs::read_to_string(self.world.join("ci-state"))
                .unwrap()
                .as_str()
            {
                "success" => CiStatus::Success,
                "failure" => CiStatus::Failure("check:failure".into()),
                "pending" => CiStatus::Pending,
                _ => panic!("invalid synthetic CI state"),
            },
        )
    }

    async fn merge_pr(
        &self,
        number: u64,
        _method: MergeMethod,
        _message: Option<&MergeCommitMessage>,
        expected_head: &str,
        permit: Option<&DeliveryMutationPermit>,
    ) -> Result<MergeRequestResult, String> {
        assert!(permit.is_none());
        assert_eq!(number, 1);
        self.request("merge", expected_head).await;
        assert_eq!(
            std::fs::read_to_string(self.world.join("ci-state")).unwrap(),
            "success"
        );
        // Real fast-forward of the bare remote, not a made-up merge SHA.
        let previous = git(&self.origin(), &["rev-parse", "refs/heads/main"])?;
        git(
            &self.origin(),
            &["update-ref", "refs/heads/main", expected_head, &previous],
        )?;
        self.record(
            &format!("merge:{expected_head}"),
            &json!({"kind": "merge", "pr_number": number, "merge_sha": expected_head}),
        )
        .await;
        Ok(MergeRequestResult::Merged {
            merge_sha: expected_head.into(),
        })
    }

    async fn observe_merge(
        &self,
        number: u64,
        expected_head: &str,
    ) -> Result<MergeObservation, String> {
        assert_eq!(number, 1);
        Ok(
            if self
                .observed(&format!("merge:{expected_head}"))
                .await
                .is_some()
            {
                assert_eq!(
                    git(&self.origin(), &["rev-parse", "refs/heads/main"])?,
                    expected_head
                );
                MergeObservation::Merged {
                    merge_sha: expected_head.into(),
                }
            } else {
                MergeObservation::OpenSameHead { auto_merge: false }
            },
        )
    }

    fn release_dispatch_target(&self, sha: &str) -> Option<ReleaseDispatchTarget> {
        Some(ReleaseDispatchTarget {
            workflow: "synthetic-release.yml".into(),
            git_ref: "main".into(),
            head_sha: sha.into(),
        })
    }

    async fn observe_release_dispatch(
        &self,
        target: &ReleaseDispatchTarget,
    ) -> Result<ReleaseDispatchObservation, String> {
        Ok(match self.observed(&target.operation_key()).await {
            Some(value) => {
                assert_eq!(value["target"], serde_json::to_value(target).unwrap());
                let required = |field: &str| {
                    value[field]
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| format!("persisted dispatch lacks {field}"))
                };
                let head_sha = required("head_sha")?;
                if head_sha != target.head_sha {
                    return Ok(ReleaseDispatchObservation::HeadMismatch {
                        observed_heads: vec![head_sha],
                    });
                }
                ReleaseDispatchObservation::Triggered {
                    run_id: required("run_id")?,
                    status: required("status")?,
                    head_sha,
                    detail: "persisted synthetic release dispatch".into(),
                }
            }
            None => ReleaseDispatchObservation::Absent,
        })
    }

    async fn trigger_release(
        &self,
        sha: &str,
        permit: Option<&DeliveryMutationPermit>,
    ) -> Result<String, String> {
        assert!(permit.is_none());
        let target = self.release_dispatch_target(sha).unwrap();
        self.request("release", &target.operation_key()).await;
        assert_eq!(git(&self.origin(), &["rev-parse", "refs/heads/main"])?, sha);
        let artifact_head = if self.world.join("wrong-tag").exists() {
            std::fs::read_to_string(self.world.join("base-sha")).unwrap()
        } else {
            sha.into()
        };
        git(&self.origin(), &["tag", "synthetic-v1", &artifact_head])?;
        self.record(&target.operation_key(), &json!({"kind": "release", "target": target,
            "run_id": "synthetic-run-1", "status": "completed", "head_sha": sha,
            "release": {"tagName": "synthetic-v1", "isDraft": false, "isPrerelease": false,
                "publishedAt": "2026-09-08T00:00:00Z", "assets": [{"name": "synthetic-artifact.txt"}]}})).await;
        if std::env::var("CF_DELIVERY_FIXTURE_PHASE").as_deref() == Ok("crash") {
            // COMMIT above is durable in WAL; production has only intent_release.
            let receipt = local_receipt(&self.world);
            assert_eq!(receipt.state, "intent_release");
            std::fs::write(
                self.world.join("release-persisted"),
                std::process::id().to_string(),
            )
            .unwrap();
            // Only SQLite threads remain here; no git/desktop/browser child is alive.
            tokio::time::sleep(Duration::from_secs(45)).await;
            panic!("supervisor failed to kill its paused worker");
        }
        Ok("synthetic release persisted".into())
    }

    async fn verify_live(
        &self,
        sha: &str,
        _url: Option<&str>,
    ) -> Result<ObservationStatus, String> {
        let target_head = git(&self.origin(), &["rev-parse", "refs/heads/main"])?;
        let target = self.release_dispatch_target(&target_head).unwrap();
        let value = self
            .observed(&target.operation_key())
            .await
            .expect("release must exist");
        // Production publication/tag validator + actual local Git ancestry.
        // This intentionally does NOT claim asset-byte/digest validation.
        github_release_live_from_value(&value["release"], sha, |tag| {
            Ok(git(&self.origin(), &["merge-base", sha, tag])? == sha)
        })
    }
}

#[tokio::test]
async fn forge_mutations_survive_a_fresh_connection() {
    let world = tempfile::tempdir().unwrap();
    let forge = FakeForge::open(world.path()).await;
    let receipt = json!({"number": 1, "head": "synthetic-head", "kind": "pr"});
    forge.record("pr:synthetic-head", &receipt).await;
    forge.db.close().await;
    let reopened = FakeForge::open(world.path()).await;
    assert_eq!(
        reopened.observed("pr:synthetic-head").await,
        Some(receipt),
        "a fresh connection must observe the committed forge mutation"
    );
    reopened.db.close().await;
}

#[tokio::test]
async fn release_observation_reloads_all_persisted_dispatch_fields() {
    let world = tempfile::tempdir().unwrap();
    let forge = FakeForge::open(world.path()).await;
    let target = forge
        .release_dispatch_target("synthetic-exact-head")
        .unwrap();
    forge
        .record(
            &target.operation_key(),
            &json!({"kind": "release", "target": target,
        "run_id": "persisted-run-29", "status": "in_progress", "head_sha": target.head_sha}),
        )
        .await;
    forge.db.close().await;
    let reopened = FakeForge::open(world.path()).await;
    match reopened.observe_release_dispatch(&target).await.unwrap() {
        ReleaseDispatchObservation::Triggered {
            run_id,
            status,
            head_sha,
            ..
        } => {
            assert_eq!(
                run_id, "persisted-run-29",
                "observer must not invent its run ID"
            );
            assert_eq!(
                status, "in_progress",
                "observer must not invent a completed state"
            );
            assert_eq!(head_sha, "synthetic-exact-head");
        }
        other => panic!("persisted exact dispatch was not observed: {other:?}"),
    }
    let mismatch = reopened
        .release_dispatch_target("different-exact-head")
        .unwrap();
    reopened
        .record(
            &mismatch.operation_key(),
            &json!({"kind": "release", "target": mismatch,
        "run_id": "persisted-run-30", "status": "completed", "head_sha": "observed-wrong-head"}),
        )
        .await;
    assert_eq!(
        reopened.observe_release_dispatch(&mismatch).await.unwrap(),
        ReleaseDispatchObservation::HeadMismatch {
            observed_heads: vec!["observed-wrong-head".into()]
        }
    );
    let missing = reopened
        .release_dispatch_target("missing-required-fields")
        .unwrap();
    reopened
        .record(
            &missing.operation_key(),
            &json!({"kind": "release", "target": missing,
        "head_sha": missing.head_sha}),
        )
        .await;
    assert!(reopened.observe_release_dispatch(&missing).await.is_err());
    reopened.db.close().await;
}

fn init_world(world: &Path) {
    let root = world.join("root");
    if root.exists() {
        return;
    }
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]).unwrap();
    git(&root, &["config", "user.name", "Synthetic Fixture"]).unwrap();
    git(
        &root,
        &["config", "user.email", "synthetic@invalid.example"],
    )
    .unwrap();
    std::fs::write(root.join("app.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("user-notes.txt"), "initial synthetic notes\n").unwrap();
    git(&root, &["add", "app.rs", "user-notes.txt"]).unwrap();
    git(&root, &["commit", "-q", "-m", "initial synthetic commit"]).unwrap();
    let base = git(&root, &["rev-parse", "HEAD"]).unwrap();
    std::fs::write(world.join("base-sha"), &base).unwrap();
    let origin = world.join("origin.git");
    git(&root, &["init", "--bare", "-q", origin.to_str().unwrap()]).unwrap();
    git(
        &root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    )
    .unwrap();
    git(&root, &["push", "-q", "origin", "main"]).unwrap();
    let checkout = world.join("delivery-worktree");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/synthetic",
            checkout.to_str().unwrap(),
        ],
    )
    .unwrap();
    // User dirt stays on root; the real delivery operates on an already isolated worktree.
    std::fs::write(root.join("user-notes.txt"), DIRTY).unwrap();
    std::fs::write(
        world.join("root-index-before"),
        std::fs::read(root.join(".git/index")).unwrap(),
    )
    .unwrap();
    std::fs::write(
        checkout.join("feature.rs"),
        "pub fn synthetic_feature() {}\n",
    )
    .unwrap();
    std::fs::write(world.join("ci-state"), "success").unwrap();
}

fn assert_root_untouched(world: &Path) {
    let root = world.join("root");
    assert_eq!(
        std::fs::read_to_string(root.join("user-notes.txt")).unwrap(),
        DIRTY
    );
    // Read the exact index before status may refresh its stat cache.
    assert_eq!(
        std::fs::read(root.join(".git/index")).unwrap(),
        std::fs::read(world.join("root-index-before")).unwrap()
    );
    assert_eq!(
        git(&root, &["diff", "--name-only"]).unwrap(),
        "user-notes.txt"
    );
    assert_eq!(
        git(&root, &["rev-parse", "HEAD"]).unwrap(),
        std::fs::read_to_string(world.join("base-sha")).unwrap()
    );
}

fn local_receipt(world: &Path) -> DeliveryReceipt {
    let repo = resolve_repo(&world.join("delivery-worktree"), Some("main")).unwrap();
    let sha = git(&repo.root, &["rev-parse", "HEAD"]).unwrap();
    read_delivery_receipt(&repo, &sha).unwrap().unwrap()
}

async fn run_delivery(world: &Path, forge: &FakeForge) -> DeliveryOutcome {
    let result = deliver(
        &world.join("delivery-worktree"),
        DeliveryCeiling::ThroughRelease,
        MergeMethod::Merge,
        0,
        &DeliverOpts {
            title: Some("test: synthetic delivery prerequisite".into()),
            expect_branch: Some("feat/synthetic".into()),
            ..DeliverOpts::default()
        },
        Some(forge),
        Some("main"),
    )
    .await;
    result.validate_contract().unwrap();
    assert_root_untouched(world);
    result
}

fn assert_blocked(outcome: &DeliveryOutcome) {
    assert!(
        matches!(outcome.final_state.as_str(), "blocked" | "waiting"),
        "{outcome:?}"
    );
    assert_ne!(outcome.reached_state, "live_verified");
    assert!(!outcome
        .steps
        .iter()
        .any(|s| s.step == "live" && s.status == "ok"));
}

#[test]
#[ignore = "only the scoped supervisor may run this with a temporary world"]
fn delivery_fake_forge_worker() {
    let world = PathBuf::from(
        std::env::var_os("CF_DELIVERY_FIXTURE_WORLD").expect("supervised worker only"),
    );
    assert_eq!(
        std::fs::read_to_string(world.join("owner")).unwrap(),
        std::env::var("CF_DELIVERY_FIXTURE_OWNER").unwrap()
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("HOME").unwrap()),
        world.join("home")
    );
    assert!(std::env::var_os("GH_TOKEN").is_none());
    assert!(std::env::var_os("GITHUB_TOKEN").is_none());
    let phase = std::env::var("CF_DELIVERY_FIXTURE_PHASE").unwrap();
    let start_deadline = Instant::now() + Duration::from_secs(5);
    while !world.join(format!("{phase}-start")).exists() {
        assert!(
            Instant::now() < start_deadline,
            "supervisor did not attach process-tree ownership"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    if phase == "survivor" {
        // Inherits only this worker's sanitized environment and owned group/job.
        let _descendant = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", DESCENDANT, "--ignored", "--nocapture"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !world.join("descendant-ready").exists() {
            assert!(Instant::now() < deadline, "descendant did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        return; // The owner exits first; its live descendant must still be reclaimed.
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        init_world(&world);
        let forge = FakeForge::open(&world).await;
        if phase == "ci-failure" || phase == "ci-pending" {
            let state = if phase == "ci-failure" {
                "failure"
            } else {
                "pending"
            };
            std::fs::write(world.join("ci-state"), state).unwrap();
        }
        if phase == "wrong-tag" {
            std::fs::write(world.join("wrong-tag"), "synthetic").unwrap();
        }
        if phase == "recover" {
            assert_eq!(local_receipt(&world).state, "intent_release");
            assert_eq!(forge.counts().await, (1, 1, 1));
        }
        let outcome = run_delivery(&world, &forge).await;
        if phase.starts_with("ci-") {
            assert_blocked(&outcome);
            assert_eq!(forge.counts().await, (1, 0, 0));
            assert!(!outcome
                .steps
                .iter()
                .any(|s| ["merge", "release"].contains(&s.step.as_str()) && s.status == "ok"));
        } else if phase == "wrong-tag" {
            assert_blocked(&outcome);
            assert_eq!(outcome.reached_state, "release_triggered");
            assert!(
                outcome
                    .steps
                    .iter()
                    .any(|s| s.detail.contains("does not include delivery commit")),
                "{outcome:?}"
            );
            forge.assert_single_dispatches().await;
        } else {
            assert_eq!(outcome.final_state, "delivered", "{outcome:?}");
            assert_eq!(outcome.reached_state, "live_verified");
            forge.assert_single_dispatches().await;
            assert_eq!(local_receipt(&world).state, "release_triggered");
        }
        std::fs::write(
            world.join(format!("{phase}-result.json")),
            serde_json::to_vec(&json!({
                "pid": std::process::id(), "outcome": outcome,
            }))
            .unwrap(),
        )
        .unwrap();
        forge.db.close().await;
    });
}

#[test]
#[ignore = "bounded owned descendant for supervisor cleanup regression"]
fn owned_survivor_descendant() {
    let world =
        PathBuf::from(std::env::var_os("CF_DELIVERY_FIXTURE_WORLD").expect("supervised only"));
    assert_eq!(
        std::fs::read_to_string(world.join("owner")).unwrap(),
        std::env::var("CF_DELIVERY_FIXTURE_OWNER").unwrap()
    );
    std::fs::write(
        world.join("descendant-ready"),
        std::process::id().to_string(),
    )
    .unwrap();
    // Safety net for the deliberately broken red-first supervisor: self-expire
    // without touching files again; never leave a permanent process on red.
    std::thread::sleep(Duration::from_secs(6));
}

struct Worker {
    child: Child,
    tree: StdProcessTree,
    reaped: bool,
    log: PathBuf,
}

impl Worker {
    fn spawn(world: &Path, phase: &str) -> Self {
        let log = world.join(format!("{phase}.log"));
        let output = std::fs::File::create(&log).unwrap();
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", WORKER, "--ignored", "--nocapture"])
            .env_clear()
            .current_dir(world)
            .env("CF_DELIVERY_FIXTURE_WORLD", world)
            .env(
                "CF_DELIVERY_FIXTURE_OWNER",
                std::fs::read_to_string(world.join("owner")).unwrap(),
            )
            .env("CF_DELIVERY_FIXTURE_PHASE", phase)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", world.join("home"))
            .env("USERPROFILE", world.join("home"))
            .env("XDG_CONFIG_HOME", world.join("config"))
            .env("XDG_DATA_HOME", world.join("data"))
            .env("XDG_CACHE_HOME", world.join("cache"))
            .env("APPDATA", world.join("config"))
            .env("LOCALAPPDATA", world.join("data"))
            .env("TMPDIR", world.join("tmp"))
            .env("TMP", world.join("tmp"))
            .env("TEMP", world.join("tmp"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", world.join("empty-gitconfig"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("GIT_CONFIG_COUNT", "5")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", world.join("empty-hooks"))
            .env("GIT_CONFIG_KEY_1", "init.templateDir")
            .env("GIT_CONFIG_VALUE_1", world.join("empty-hooks"))
            .env("GIT_CONFIG_KEY_2", "credential.helper")
            .env("GIT_CONFIG_VALUE_2", "")
            .env("GIT_CONFIG_KEY_3", "commit.gpgSign")
            .env("GIT_CONFIG_VALUE_3", "false")
            .env("GIT_CONFIG_KEY_4", "tag.gpgSign")
            .env("GIT_CONFIG_VALUE_4", "false")
            .stdin(Stdio::null())
            .stdout(output.try_clone().unwrap())
            .stderr(output);
        #[cfg(windows)]
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            cmd.env("SystemRoot", system_root);
        }
        isolate_std_process_tree(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = match StdProcessTree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("could not own worker tree: {error}");
            }
        };
        // The worker cannot spawn git until its group/job is owned.
        let worker = Self {
            child,
            tree,
            reaped: false,
            log,
        };
        std::fs::write(world.join(format!("{phase}-start")), "owned").unwrap();
        worker
    }

    fn wait(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if self.exited_without_reaping().unwrap() {
                let status = self.terminate_then_reap().unwrap();
                self.assert_reaped();
                assert!(
                    status.success(),
                    "worker failed: {}",
                    std::fs::read_to_string(&self.log).unwrap()
                );
                return;
            }
            assert!(
                Instant::now() < deadline,
                "worker deadline: {}",
                std::fs::read_to_string(&self.log).unwrap()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(unix)]
    fn exited_without_reaping(&mut self) -> std::io::Result<bool> {
        assert!(
            !self.reaped,
            "an already reaped PID grants no group-signal authority"
        );
        // WNOWAIT preserves this owned child's PID even when it is a zombie.
        // Therefore the PGID cannot be re-used until after our group cleanup.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }

    #[cfg(not(unix))]
    fn exited_without_reaping(&mut self) -> std::io::Result<bool> {
        // On Windows the Job kernel handle, not a reusable PID, owns the tree.
        Ok(self.child.try_wait()?.is_some())
    }

    fn terminate_then_reap(&mut self) -> std::io::Result<std::process::ExitStatus> {
        assert!(
            !self.reaped,
            "never signal a group after releasing its leader PID"
        );
        #[cfg(target_os = "macos")]
        let no_signal_needed =
            self.exited_without_reaping()? && self.only_reserved_leader_remains()?;
        #[cfg(not(target_os = "macos"))]
        let no_signal_needed = false;
        if !no_signal_needed {
            self.tree.terminate(&mut self.child)?;
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.exited_without_reaping()? {
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "terminated worker did not exit",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }

    #[cfg(target_os = "macos")]
    fn only_reserved_leader_remains(&self) -> std::io::Result<bool> {
        assert!(!self.reaped);
        // Darwin refuses SIGKILL with EPERM when the group contains only its
        // zombie leader. Ask libproc for this exact still-reserved group, not
        // all system processes. A full/failed observation is not permission.
        let mut pids = [0_i32; 256];
        let count = unsafe {
            libc::proc_listpgrppids(
                self.child.id() as i32,
                pids.as_mut_ptr().cast(),
                std::mem::size_of_val(&pids) as i32,
            )
        };
        reserved_leader_is_only_member(count, &pids, self.child.id())
    }

    fn assert_reaped(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.tree.active_process_count(&mut self.child).unwrap() != 0 {
            assert!(
                Instant::now() < deadline,
                "worker process tree was not reclaimed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !self.reaped {
            // No Unix path may call Child::try_wait first: even normal owner
            // exit retains its PID until this owned group has been terminated.
            let _ = self.terminate_then_reap();
        }
    }
}

fn supervised_world() -> tempfile::TempDir {
    let world = tempfile::Builder::new()
        .prefix("cf-delivery-forge-")
        .tempdir()
        .unwrap();
    for name in ["home", "config", "data", "cache", "tmp", "empty-hooks"] {
        std::fs::create_dir(world.path().join(name)).unwrap();
    }
    std::fs::write(world.path().join("owner"), uuid::Uuid::new_v4().to_string()).unwrap();
    std::fs::write(world.path().join("empty-gitconfig"), "").unwrap();
    world
}

fn result(world: &Path, phase: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(world.join(format!("{phase}-result.json"))).unwrap())
        .unwrap()
}

fn close_world(world: tempfile::TempDir) {
    let path = world.path().to_path_buf();
    world.close().unwrap();
    assert!(
        !path.exists(),
        "only this owned world must be fully removed"
    );
}

#[test]
fn ci_not_green_never_merges_or_releases() {
    for phase in ["ci-failure", "ci-pending"] {
        let world = supervised_world();
        Worker::spawn(world.path(), phase).wait();
        assert_eq!(result(world.path(), phase)["outcome"]["pr_number"], 1);
        close_world(world);
    }
}

#[test]
fn wrong_release_tag_head_never_claims_live() {
    let world = supervised_world();
    Worker::spawn(world.path(), "wrong-tag").wait();
    close_world(world);
}

#[test]
fn same_identity_retry_reopens_persistent_forge_without_duplicate_mutations() {
    let world = supervised_world();
    Worker::spawn(world.path(), "success").wait();
    Worker::spawn(world.path(), "retry").wait();
    let first = result(world.path(), "success");
    let second = result(world.path(), "retry");
    assert_ne!(first["pid"], second["pid"]);
    for field in [
        "branch",
        "commit_sha",
        "pr_number",
        "pr_url",
        "release_receipt",
    ] {
        assert_eq!(
            first["outcome"][field], second["outcome"][field],
            "identity drift: {field}"
        );
    }
    close_world(world);
}

#[test]
fn preexisting_dirty_file_survives_delivery() {
    let world = supervised_world();
    Worker::spawn(world.path(), "success").wait();
    close_world(world);
}

#[test]
fn supervisor_reclaims_owned_descendant_after_main_worker_exits() {
    let world = supervised_world();
    let mut worker = Worker::spawn(world.path(), "survivor");
    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.wait()));
    let survivor_after_wait = worker.tree.active_process_count(&mut worker.child).unwrap() != 0;
    // Red-first safety: the old implementation has already reaped the leader.
    // Never signal that possibly reusable PGID. Let our fixed-duration child
    // self-expire, then clean this exact temp world and only then fail the test.
    let safety_deadline = Instant::now() + Duration::from_secs(8);
    while worker.tree.active_process_count(&mut worker.child).unwrap() != 0 {
        assert!(
            Instant::now() < safety_deadline,
            "bounded descendant did not self-expire"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(worker);
    close_world(world);
    assert!(
        waited.is_ok() && !survivor_after_wait,
        "exited main worker must not exempt owned descendants from cleanup"
    );
}

#[test]
fn hardkill_after_forge_release_commit_reconciles_before_any_redispatch() {
    let world = supervised_world();
    let mut worker = Worker::spawn(world.path(), "crash");
    let old_pid = worker.child.id();
    let deadline = Instant::now() + Duration::from_secs(40);
    let marker = world.path().join("release-persisted");
    while !marker.exists() {
        assert!(
            !worker.exited_without_reaping().unwrap(),
            "worker exited before crash point: {}",
            std::fs::read_to_string(&worker.log).unwrap()
        );
        assert!(
            Instant::now() < deadline,
            "no crash marker: {}",
            std::fs::read_to_string(&worker.log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(
        std::fs::read_to_string(marker)
            .unwrap()
            .parse::<u32>()
            .unwrap(),
        old_pid
    );
    assert!(!worker.terminate_then_reap().unwrap().success());
    worker.assert_reaped();
    assert!(
        !world.path().join("crash-result.json").exists(),
        "no completed local outcome may exist at crash point"
    );
    drop(worker);
    Worker::spawn(world.path(), "recover").wait();
    assert_ne!(result(world.path(), "recover")["pid"], old_pid);
    close_world(world);
}
