# M115（M108/M111 同族）：合并队列/GitHub update-branch 生成的新头必须被识别为同一交付并继续观察其 CI

## 现象（真机 2026-10-18 13:04–13:52，v1.88.24）
- 会话 7a020c21，PR #728：会话推送 0357e6fe 并 armed。合并队列为满足 strict 对 PR 执行 update-branch，生成合并提交 aacde6dd（父提交 = 0357e6fe + main）。该头上 check-rust 偶发失败。
- objective_events 的 canonical_pr_progress 最后一次在 13:04（head 仍记 0357e6fe），之后观察遇到 HeadChanged → Unknown → continue，不再更新；M108 的「CI 失败唤醒」因此从未触发，会话空等 57 分钟，编排方手工重跑 CI 并去掉 blocked 标签。

## 需求
- R1 用数据复现时间线并定位：canonical PR 观察与 CI 唤醒在 head 被 update-branch 改变时走了哪条分支。
- R2 若 PR 新头是「以会话已知头为父之一、另一父在 main 上」的合并提交（GitHub update-branch / 合并队列同步），视为同一交付：更新 canonical_head_sha，继续观察该头的 CI；CI 失败照 M108 唤醒会话（附失败测试名与是否疑似偶发）；全绿保持 armed。其他陌生头仍失败关闭。
- R3 被唤醒后会话修复推送时，以新头为基础（先 fetch 合入），不得因为本地不认识合并头而报 identity 冲突。
- R4 测试（先失败）：合成 forge 上 update-branch 生成合并头 → CI 失败 → 会话被唤醒；陌生头（父不含会话头）→ 仍失败关闭。
## 约束
- 不放宽外部写入安全边界。合成 fixture，不写生产 DB。
