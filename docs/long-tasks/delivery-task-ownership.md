# 交付归属闭环

## Basics
- Task ID: CF-OWN-20261112
- Title: #1066 R2 任务归属与交付目标树
- Feature spec: `docs/specs/feature-specs/delivery-task-ownership.md`
- Related Req IDs: OWN-R1..R5

## Completion Standard
- Done means: 红绿回归、生产调用链与本地 fake remote、独立审查和正常本地门禁完成；云端与 Mac 验证等级单列。
- Blocked means: 明确权限/环境/可信证明依赖仍缺失，不伪造通过或扩大授权。

## Current State
- Current phase: 本地实现及独立复验，not live。
- Current checkpoint: main `989c8da9`，独立 `fix/delivery-task-owned-tree`。
- Next owner: 当前项目会话保留未完成候选；Mac 原生混合验收由父会话的原 Mac Codex 会话接续。
- Updated at: 2026-11-12

## Completed Items
- [x] 排重：无同类本地分支或打开 PR，保留 PPT/安全/PM。
- [x] 实际读取当前仓库 Harness；缺少全局 Mac 技能，不声称自动继承。
- [x] 红例：production deliver + 本地 bare remote 4 项失败，thin stage 1 项失败。

## Remaining Items
- [x] 可信基线与精确文件回执证明、固定内容交付及并发守卫。
- [x] 12 项真实 native 回执/生产交付及拒绝回归通过；SQLite 文件重开不是跨进程。
- [x] 受影响完整回归及最终独立审查；435 个唯一测试，432 通过、3 个同环境 main 基线失败，非全绿。
- [ ] 正式可信场景登记与目标平台验收。
- [ ] Mac 原生验收，Mac 可用但由父会话独立验证方执行，本执行器无直接调用入口。

## Blockers
- 本批无真实远端写入授权，不 push/PR/merge/release。
- 历史或通用 shell 改动未产生精确回执时 fail closed；这是兼容收紧，不宣称所有旧交付都可自动继续。
- thin 普通 hooks 固定树提交已实测；通用脚本/编译器产物、无来源历史内容、后台 thin task 身份仍不具备自动归属路径。当前候选不是完整兼容修复。

## Evidence
- `/workspace/delivery-scope-evidence/` 保存云端命令、红绿日志、独立审查与 Harness 继承核查。
- Mac 原始规划证据来自父会话转述，云端未读取其本地目录；云端另行执行生产链，不混称 Tauri GUI。

## AI Collaboration
- context scope: 任务工作区、精确文件副作用回执、legacy/thin 交付与恢复。
- assumptions: 内容归属证明不替代独立远端授权或运行中撤权门禁。
- review point: 独立来源链、foreign preimage、index 保存、固定 SHA 推送并发审查。
- validation result: 红测 4 项 production + 1 项 thin 已执行；初轮 8 项通过，修复 SQLite 文件夹具后新增 12 项全部通过；thin 24 项通过。旧完整 legacy 137 过/42 失败，tools 55 过/1 失败；真实恢复夹具迁移后精确测试 1/1 通过；未重新把整个旧套件报告为通过。

## Stop Boundary
- 只本地实现、测试与可审查提交；不触碰真实应用数据库，不改信任根/保护规则，不混入其他候选。

## Compatibility Decision（历史 tree 3c971d26 / 033740d4）
本候选不具备完整正常交付兼容性，不能交付为完成。shell 产物、历史授权变更、删除/rename 仍缺精确归属证明；需要受控写入日志或宿主显式批准固定清单。38 项旧交付断言被新门禁提前阻断，3 项凭据存储错误未做 pristine main 对照。不得以全拒绝、虚构 proof 或降级门禁掩盖这些缺口。

## 2026-11-12 Mac 对照后的兼容性切片

- 父会话 Mac 独立回执：归档和 tree 完全匹配；R2 12/12、thin 24/24、恢复 1/1 通过；候选 legacy 141/38，纯 main 989c8da9 同机 167/0。38 项为真实候选回归；云端3项凭据失败在Mac不出现。
- 云端新增两项真实生产工具兼容测试：native write/edit + 任务预暂存正常交付通过；bash 正常执行和文件内容正确，但精确 file receipts 为0，交付正例保持失败。未将断言改成拒绝，未批量豁免旧38项。
- 本轮仅增加 cfg(test) 夹具/测试及 `docs/design/delivery-content-adoption-proposal.md`；产品实现未改。旧 Library 候选包保持原字节。
- 新方案通过只读独立设计审视：内容采用的独立purpose、签名/expiry/Stop、可信基树构造、原子消费回执、index锁/HEAD CAS均需实现；不能冒用现有“允许交付”带来远端授权。
- 下一最小可用切片是宿主固定清单＋独立审批＋真实UI＋采用回执的完整接线；thin普通hooks固定树事务另需实现。任意shell无人确认自动归属需要私有执行写层，现有实时工作区沙箱不能证明来源。
- 本阶段仍为not live，兼容性修复未完成。无真实远端写入、保护变更或生产DB访问。

## 后续技术路径（当前工作树，尚未形成最终验收候选）

前一节记录的是冻结 tree `033740d4` 的历史状态，不能当作后续产品实现。用户随后要求优先技术归属路径；未实施内容采用 UI 或新增用户授权语义。

- 已实现有界 POSIX 字面量命令的执行前精确意图，包含读依赖整组证明；不是共享目录前后差分，也不是通用 shell/编译器隔离。首批 shell 14 项、原 R2 12 项、兼容正例 2 项通过，尚需最终候选复验。
- 新增逐次真实宿主派发的内容观察，关联活动 foreground run、Objective revision、登记工作区与权限变化版本。它不授予权限、不改变 thin 恢复状态。新增 SQLite migration 0027；持久状态变化、升级兼容和恢复拒绝须单独验收，不能只凭首次写入成功宣布完成。
- fresh thin 与新 hooks 夹具改用生产 `storage::db::connect`，执行完整 migrations 与 ensure_schema；不再按失败逐个补列。thin 在首次写入前启用，且旧 binding/receipt 表存在、记录为零。完整初始化下首次 native 写入来源证明 1/1 通过。
- supplemental shell 内容证明不能使整个命令自动重放或虚构成功；两项实际恢复回归先红后绿 2/2。
- thin 通过持久本地 owner、正常 Git hooks 准备精确 child、index lock/ref CAS 安装，未绕过 hooks。当前第三轮 8/9：剩余并发夹具继承 Git 索引定位环境，尚未正确触发用户索引并发；该失败不算产品绿灯，正修正夹具后重测。
- 所有上述仅本地合成仓库/SQLite/本地 bare origin。最终全量、独立审查、Mac 新候选及 Windows 尚未完成；PPT、安全与 PM 工作树未改。

## 最终本地回执（2026-11-12，未推送）

- 基线 main `989c8da9c272de73142099ef25ed2eb7e3daa88b`，最后只读 fetch 未变化。候选仍未提交，精确树通过独立索引冻结并随私有验收包交接，不覆盖原有暂存区。
- 最终受影响组：tool_backend 81/81；shell_intent 6/6；tool_recovery 11/11；legacy delivery 191过/3失败；tools delivery 56/56；thin 39/39；delivery_run 48/48。额外两项 shell recovery 重复筛选通过，已含在81项中，不重复计数。唯一测试435项：432通过、3失败。
- 同环境原版 main 全 legacy 164过/3失败，失败均为三个OS凭据写入后读回用例，与候选相同。38项旧兼容回归全部通过、237处原断言保留。未将基线失败标成通过。
- 最后新增 Stop 先红后绿：持久停止后实际 bare push曾增加引用；最终同一前台gate贯穿local/remote，固定run，旧运行不能借同root新运行。正式取消夹具使用ObjectiveStore::cancel_chat_exact；生产数据库约束未改弱。
- 源码观察器测试含实际工具、SQLite、Git和本地bare；部分取消/崩溃是prepare/finish故障注入，不称完整桌面或进程被杀重启E2E。Windows、当前候选Mac、GUI、真实provider/远端CI、旧binary回退未执行。
- 治理baseline静态检查通过；受信registry只读映射诊断发现14个未映射文件，未改registry/validator/保护规则。未提交的树不能借HEAD=main的空diff门禁获得认证，正式场景门禁仍待治理登记。
- 独立审查已闭合来源读取依赖、未知shell恢复、hooks来源/歧义intent、Stop跨阶段run重绑和非阻塞文件打开问题。worktree不是安全沙箱；hook摘要不证明全部传递依赖/环境。
- 下一步：Mac按新包固定树复跑；独立审核并正式登记场景；通用生成器/历史内容必须继续解决可信私有写层或另行审核正常应用内内容采用流程，不能借已有交付授权自动采用。PPT、安全、PM成果保持不动。

## Mac独立验收与通用路径研究（2026-11-12）

- 父会话回传精确tree66d1cdff：Mac仓库cargo-shared/offline七组435/435；另保留原独立断言的native shell与预暂存2/2通过，applied receipts=2、TaskDeliveryScope=Ok。测试后恢复精确候选；未远端写入。此为外部独立回执，不是本执行器重跑，非Windows/GUI/provider。
- 本轮产品Rust/SQL未变、旧归档不变。新增普通设计文档delivery-general-output-next.md与执行器外合成实验，未修改可信规则或新增产品可达入口。
- 独立审查确认现有live Docker挂载、Mac宽读取/共享缓存、Windows Job生命周期均不足以自动证明通用产物来源。可复用journal/Stop/hooks，不另造调度器。
- 实际私有层实验完成Python生成+C编译、宿主停止后收集及链接拒绝；同时实证共享Docker控制端可注入停止层，不能签发可信来源。完整broker/封存/并发落地与原生Windows仍未实现/未验收。
- 下一步按新文档切片：collector/不可变候选与冲突协议→一个可信平台adapter→现有hooks联合→独立Windows验收。适配器缺失时的正常应用内固定内容采用流程仅供审核，不借普通交付权限自动采用。14文件注册障碍保留；无push/PR/合并/发布。
