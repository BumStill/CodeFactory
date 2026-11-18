# M283 本轮恢复核对

- 重复的 model-availability、thin-continue、thin-repair 唤醒只作为恢复提示，不视为新的权限授权。
- 现场查询：PR #1075 为 OPEN、isDraft=true、labels=[]；未将旧的“检查全绿”提示作为可合并依据。
- 工作区核对时 git status --short 为空。
- 原失败回归测试 1/1、权限网关测试组 38/38 已通过，并已记录在 PR 评论；不代表 M283 整体验收通过。
- `repair_session_split_locked` 会将 donor 的 chat_turn_state.objective_id 改为 keeper，并将 donor Objective 写为 legacy_orphan。尚需独立回归测试覆盖仍在 working 且 terminal_reason 为空的回合，不得在测试前宣称根因修复。
- 回合真实建表位置：src-tauri/migrations/0003_session_execution_governance.sql、src-tauri/src/storage/db.rs；不要猜测 chat_turn.rs 等路径。
- 本轮多次 edit_file 替换请求被参数校验拒绝：`send either edits (batch) or old_string/new_string (single) — not both`。这些调用均非成功写入，未新增测试、未修改产品代码。
- 仍待完成：先增加并执行失败测试，再实现活执行者身份保护；核验权限对齐失败的 owner、唤醒保留、救援后的派生状态与主动停止反例。完成前保持草稿，not live。
