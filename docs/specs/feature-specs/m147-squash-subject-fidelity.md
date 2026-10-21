# M147 Squash 主题保真：陈旧 PR 标题不得改写批次发版类型

## 问题

合并队列用 PR 标题作为 squash 提交的主题。PR 标题一旦与基线分支上**已存在的**提交主题相同（典型情形：PR 标题沿用了上一条 `chore: bump version to x.y.z (#NNN)`），真正落地的改动就会被记成 `chore`：

- 发版节奏门禁按主题判定 `feat`/`fix`，整批被判为「无 feat/fix」而不切版；
- 已发布的版本号与内容错位，用户看到「修好了但没发出去」；
- 事后只能靠人肉翻 PR body 才看得出这批其实带了修复。

实例：PR #821（M146 收尾回填与唤醒准入）以 `fix(chat)` 交付，落到 main 的主题却是 `chore: bump version to 1.91.15 (#820)`。

## 行为规格

`resolveSquashSubject(title, messages, baseSubjects)`：

1. 主题比较前做规范化：去首尾空白、折叠连续空白、小写、去掉结尾的 `(#数字)` PR 后缀。
2. 标题规范化后**不**在 main 已有主题集合中 → 原样使用，绝不改写。
3. 标题命中已在 main 的主题 → 退回该 PR 自己最新的、且不命中 main 的提交主题（`feat`/`fix` 得以保留，批次发版类型不再被掩盖）。
4. PR 自己的提交同样全部命中（例如真正的版本号 bump PR 重跑）→ 保留原标题，不臆造主题。
5. 标题为空 → 用 PR 最新提交主题；仍为空才退到 `fix: synthetic change`。

真实合并路径（`createGhAdapter().merge`）：

- 用 `git log -5 --format=%s origin/main` 取基线主题集合；读取失败时按空集合处理（保守：不改写标题）。
- 发生替换时在 PR 上留一条说明性评论，记录原标题与被采用的主题，保证可审计。
- 审计评论自身失败不得阻断合并。

## 验收

- `node --test scripts/merge-queue.test.mjs`：5 个 M147 用例覆盖「沿用基线主题」「准确标题不被改写」「多个提交取最新非陈旧主题」「无更好候选时保留候选」「空标题兜底」。
- 既有队列用例全部保持通过（79 项）。
- 场景：`CXD-002`（交付证据链决定完成），`scripts/merge-queue.mjs` 属其 `change_patterns`。

## 不做的事

- 不改写用户明确给出的、与基线不冲突的 PR 标题。
- 不在队列里推断发版类型：主题保真即可，发版判定仍归 `tools/release/plan_release.py`。
