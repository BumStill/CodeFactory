# 交付内容的任务归属证明（#1066 R2 补全）

## 问题
`prepare_scoped_commit` 以真实 index 为起点、追加 tracked 修改和非黑名单 untracked。这样会继承外来暂存文件，也会吸收工作区后来出现的普通无关文件。大小守卫和规划前后摘要只能证明体积/稳定性，不能证明归属。Mac 已提供生产规划函数级复现；云端需独立复验生产调用链，不能把该证据称为完整 Tauri 交付验收。

## 需求
- OWN-R1：任务绑定和基线来自宿主持久记录；调用者参数、文件名黑名单、分支名或单纯 base diff 不能授予文件归属。
- OWN-R2：复用现有逐文件副作用回执，从可信基线到目标内容形成连续、完整、同任务的变更证明。未知归属、失效绑定、旧回执及摘要不匹配均 fail closed。
- OWN-R3：覆盖 tracked 修改/删除、合法新文件、预暂存、重命名、未跟踪文件和已提交的分支差异。没有证明的新文件不能自动纳入；外来暂存不能因过滤而丢失。
- OWN-R4：拒绝时保留真实 index 原字节、HEAD、工作树和所有未知文件；在实际变更前复核并发状态，恢复不得沿用失效证明。
- OWN-R5：legacy/thin 等生产交付入口都遵循同一归属边界，不能由另一 stage 路径绕过。隔离测试通过真实生产规划/调用链和本地 fake remote 检查目标树，记录证据等级。

## 验收与兼容
先红后绿、独立审查；合法来源与拒绝样本同测。无法证明历史或通用 shell 改动时明确阻止自动交付，不把缺失回执转换成授权。临时 DB 与 bare Git fixture 不触碰真实应用数据库或真实远端。

Applicable Harnesses：Spec、Compatibility、Payload、Observation、AI Collaboration；Release 只交接，不含发版操作。继承仓库 AGENTS/统一门禁；Mac 全局技能未在云端出现，不声称完整继承。云端生产函数/进程/数据库测试与 Mac 原生完整交付验收分开报告。

本批只允许本地实现与测试；不 push、不 PR、不合并、入队、发版、部署，不修改保护或信任根。PPT、安全 #1043、PM 工作保持独立。

## 当前技术实现边界

- OWN-R2/R3：native write/edit 与可完整预测的 POSIX 字面量文件命令保存执行前精确意图、全部读取依赖及真实成功后的观察。内容证明不是权限许可或执行隔离。未知命令、外来输入、缺少历史来源链仍不能自动认领；没有实施新的用户内容采用流程。
- OWN-R2/R5：fresh thin 使用真实 foreground run/Objective/workspace 绑定的逐次内容记录，不要求 legacy binding/receipt。独立派发 UUID 保留重复命令的不同输入；Stop、权限变化版本、run/revision 改变或未知结果不得结算为 applied。未接入的后台 task 身份不能借用 chat 身份。
- OWN-R4/R5：thin 固定树提交须执行正常 Git hooks，以持久本地提交 owner 记录准备与安装，index lock 与固定旧 OID CAS 保留并发守卫。Objective 仍是业务状态权威；本地完成不得等同 PR 完成。hook 结果未知时不自动重放；临时工作树不是操作系统沙箱。
- 生产数据库测试使用 `storage::db::connect` 完整迁移与 schema 初始化；“fresh thin”要求旧回执记录为零，不允许以缺少生产表/列代替。
