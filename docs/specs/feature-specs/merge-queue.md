# Feature spec: Merge queue — green PRs queue and merge in order

## Background and user decision

Continuous merging is a top-level principle across repos
(`docs/principles/release-cadence.md`). Several PRs often finish around the same
time; nobody should have to babysit "catch up → rerun → merge" while they wait,
and the guarantee that main is always green must not drop.

## Decision

PRs that are green and meet the merge conditions enter a queue. The system
merges them one at a time in order, verifying each against "latest main + the
PRs ahead of it"; nobody catches up by hand. Every commit that lands on main has
passed the required checks on that exact combined state.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-MQ-R1 | A PR that meets the merge conditions (required checks green, governance passed, delivery authorized, no hold) enters the queue on its own and merges in order; nobody manually catches up or triggers a rerun | End-to-end: two PRs green at the same time → both merge in order, zero manual steps |
| CF-MQ-R2 | **Not weakened**: every commit that lands on main has passed every required check on "that commit's exact content" (combined with latest main); there's no window where "it passed against an old base, so merge it" | Counter-example test/drill: a PR ahead in the queue changes something that conflicts with the one behind → the one behind is verified on the combined state and fails, and is not merged |
| CF-MQ-R3 | The governance gates keep working in queue mode: the scenario gate's trusted-base validation, governance baseline, and release gates all run against the queue's combined state and still fail closed; a candidate can't certify itself | Tests + one drill record |
| CF-MQ-R4 | A failing PR leaves the queue with a plain-language reason (which check, what failed); the PRs behind it keep going | Test |
| CF-MQ-R5 | Release flow compatibility: the version-bump PR opened by auto-release can also go through the queue, with no deadlock against "binding to the exact main head" (`expected_head_sha`) or `Release-Urgency` / hold judgements | Drill: one release while a PR is still in the queue |
| CF-MQ-R6 | CodeFactory's delivery tool (`ceiling: merged`) uses the queue instead of looping on catch-up / rerun; a stopped task must not keep merging through the queue in the background (see M47: on 10-09 #584 was still merged by the delivery tool after its session was stopped) | Integration test |
| CF-MQ-R7 | The documentation (`AGENTS.md` and `docs/principles/release-cadence.md` merge sections) matches the new flow | Doc diff |
| CF-MQ-R8 | 队列在一次运行的合入批次里读到 `Release-Urgency: immediate`，就按发布节律切版；`hold`/非法 trailer 不切并留痕；dispatch 失败可见且队列运行非零退出；门禁（ruleset、strict、auto-release 输入语义）不被削弱 | 单元测试 + 队列运行日志 + 一次真实批量合入后的切版记录 |

## Applicable Harnesses

Spec Harness; Release Harness; Compatibility Harness (existing
rulesets/workflows/delivery records); Observation Harness (queue state and each
merge are traceable); AI Collaboration Harness. **This task is a governance
change**: load `docs/repo-governance-profile.md` as quick-profile requires.

## Test matrix

- Normal: 1 PR / 2 PRs green at the same time / 3 PRs in a row.
- Conflict: the PR ahead changes the same lines as the one behind; the one
  behind fails on the combined state.
- Failure: a check fails for one PR in the queue → it's removed, the others
  continue.
- Release: auto-release opens the version-bump PR while other PRs are in the
  queue.
- Stop: the task is stopped mid-queue → nothing merges in the background.

---

## Implementation Notes (does not change any requirement)

### Why the queue is repo-side, not GitHub-native

Verified 2026-10-09 with `gh api graphql { repository(...) { mergeQueue(branch:"main") } }`:

- The repository is **personal-account owned** (`owner.type == "User"`).
- GraphQL `repository.mergeQueue` returns **`null`**.

GitHub's native merge queue (`merge_group`) is only offered to organization
repositories, so this repo needs an equivalent that satisfies CF-MQ-R1…R6. The
equivalent is a **repo-side merge-queue orchestrator** (`scripts/merge-queue.mjs`)
driven by a trusted-runner workflow (`.github/workflows/merge-queue.yml`).

### The ruleset is the R2 enforcement point, and it is not weakened

Ruleset `main-pr-and-ci-gate` (id `20222077`) carries
`required_status_checks.strict_required_status_checks_policy = true` and six
required checks. That "up-to-date" requirement is exactly what makes R2 true:
GitHub refuses to merge a PR whose head is not built on the latest `main`. The
queue therefore does **not** disable it; the queue *performs the catch-up
automatically* (update-branch → required checks re-run on the combined state →
squash merge). Only the head PR of the queue ever needs a re-run, and no human
triggers it. See the PR "Ruleset change plan" section for the current/target
values and rollback.

### Failure semantics (R4) and stop semantics (R6)

- A PR whose required check fails on the combined state is **removed from the
  queue** and labelled with a plain-language reason naming the check; the queue
  continues with the next PR.
- A **queue arm marker** (`merge-queue: armed` label + recorded expected head)
  is what authorizes the queue to merge a PR. A stopped task clears the marker,
  so nothing merges in the background after the session ends (fixes M47).

### immediate 切版（M120）

`Release-Urgency: immediate` 是「合入后立刻切版」的信号。PR 改由本队列合并后，
这个信号不能只停留在提交里，否则 immediate 实际失效（2026-10-19：M120 证据里
#749、#751 带 immediate 合入，直到下一班次都没有发版）。

一次队列运行（一次 drain）的规则：

1. 每成功 squash 合并一个 PR，读取**落在 main 上的那个提交**的提交消息最后一段，
   只认最后一段里的 `Release-Urgency:` trailer（与 `tools/release/plan_release.py`
   的 footer 解析一致）。正文中间出现的同名行不算信号。
2. 本批次出现 immediate 时，先按 `docs/principles/release-cadence.md` 扫描
   `<上个 tag>..main`：若批次含 `Release-Urgency: hold` 或非法 urgency，则不切版，
   在每个 immediate PR 上留明文评论说明被哪一类 trailer 拦住，并给出
   `allow_guarded_batch=true` 的显式放行路径。
3. 否则 `workflow_dispatch` `auto-release.yml`，`expected_head_sha` 固定为本批次
   最后合入的 main head；同一轮多个 immediate 只 dispatch 一次，避免重复切版。
4. dispatch 之前任何一步失败（读提交、扫描、dispatch）都在 PR 留明文评论，并让
   队列运行以非零退出，不允许静默吞掉。
5. 门禁不变：不碰 ruleset、不关 strict，`auto-release` 的 `hold` / `force` /
   `allow_guarded_batch` 语义原样保留；队列只负责「按节律触发」，不替发布流程
   做决定。

回滚：删除队列运行末尾的这一步，即恢复「只保留 trailer、由人工 dispatch」的旧行为。
