# M180（紧急）：v1.91.38（含 M177）信任模式下纯只读组合命令仍弹审批

## 证据
10-26 20:2x，会话 ae19592c（trusted，授权交付，cwd=本会话托管工作区），app v1.91.38（含 M177 #878），待审批命令原文：
```
sed -n '175,244p' src-tauri/src/agent/usage_windows.rs; sed -n '223,257p' src-tauri/src/vision/mod.rs; sed -n '35,100p' src-tauri/src/agent/internal_text.rs; grep -n 'attach_usage_settings' src-tauri/src/agent/mod.rs src-tauri/src/agent/model_transport.rs; grep -n 'settings' src-tauri/src/agent/internal_text.rs | head -15; git worktree list; git log -1 --oneline; git log -1 --oneline origin/main
```
全部只读。M172/M177 均声称已修且有真机验证，但这条仍进入审批——说明测试覆盖的是挑选过的命令，判定路径仍有缺口。
## 需求
- R1 **先在真实 app 判定入口上原样跑这条命令**，打印每一段的判定结果，找出触发审批的那一段（sed -n？git worktree list？`;` 分隔？`| head`？），写进 PR。
- R2 修复后，把 10-23~10-26 所有被误拦的真实命令（见下）作为数据驱动回归集，全部断言在 trusted+本会话工作区下 Allow：本条；`git fetch --prune origin main && git stash push -u -m x && git merge origin/main && git stash pop`；`git checkout origin/main -- <path> && git diff --stat && git status --short && sed -n …`；`git add -A && git commit -m …`；`node -e '…require("./docs/…")…' | head -20`；`git fetch … ; find … ; grep -R … | head -90; ls …; command -v pnpm; gh auth status`。
- R3 反例仍审批：`gh auth token`、`gh pr merge`、`git push --force`（非 with-lease 到本会话分支）、越界路径写入、`rm -rf /`。
- R4 端到端：用 dispatch 起一个 trusted 会话让它执行 R2 全部命令，断言 list_approvals 为空（附证据）。
- R5 默认策略：trusted 下无法静态判定但不含 R3 危险片段的本地命令默认放行（遵循 M130 两档权限），而不是默认审批。
## 约束
- task-template；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes

按任务模板 §8，真实安装版验收由编排方发版安装后完成，会话内使用生产权限入口的集成测试。保持 M159 本地改写快照与远端/越界安全边界，不以默认放行跳过这些边界。

Applicable Harnesses：Spec、Compatibility、Payload、Observation、Release、AI Collaboration。无 UI 布局修改。
