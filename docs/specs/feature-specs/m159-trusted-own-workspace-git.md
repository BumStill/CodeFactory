# M159：信任模式下，会话在自己的托管工作区里做 git 撤销/清理不再逐条要审批

## 证据（10-22，编排方当天人工批准 5 次）
- c95cb8a5：`git reset --hard adbdd130`（回到自己已推送的提交）、`git restore --staged … && git clean -fq <自己新建的文件>`。
- adb96f9b：`git checkout -- . && rm -rf src-tauri/src/vision …`（删除另一个自己托管工作区里的重复副本）、`git reset --hard 3c2a9c66`。
- 会话均为 permission_mode=trusted、delivery_authorized；目标路径都在该会话自己的 managed workspace（execution_workspaces.session_id=本会话）内，且数据可从已推送提交 / 另一工作区恢复。
## 需求
- R1 信任模式下，作用域完全落在**本会话登记的托管工作区**内的 git 本地改写（reset/restore/checkout -- ./clean/stash/rm 工作区内文件）直接放行，不弹审批；在工具卡片上标注「仅影响本任务工作区」。
- R2 仍需审批：路径越出本会话托管工作区（含用户主仓库、其他会话工作区、$HOME 其他位置）；push --force / 删除远端分支 / 改写已推送历史；会丢弃**未推送且不在任何其他提交/工作区中**的改动时（先自动备份到 refs/codefactory/backup/<ts> 再放行，可视为满足）。
- R3 标准模式行为不变。
- R4 测试（先失败）：上面 4 条真实命令各一个 fixture（信任模式→放行）；越界路径与 force-push 反例（→ 仍审批）；未推送改动被 reset 前生成备份 ref。
## 约束
- task-template；合成 fixture；不削弱 R2 列出的保护。授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes

- 自动放行仅接受完整可证明的命令白名单；未知语法保留审批。备份同时保留 HEAD 父链、暂存区和工作目录（包括未跟踪文件）。备份失败保留审批。
- Spec、Compatibility、Payload、Observation、Viewport、AI Collaboration Harness；交付止于合并，不发版。真机验收由编排方发版后完成。
- R1/R4：四条合成 Git fixture；R2：越界、符号链接、远端改写反例与真实备份恢复；R3：标准模式回归。
