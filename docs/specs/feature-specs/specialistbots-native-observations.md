# SpecialistBots 应用内只读取证与持久观察

## 主路径

项目工作区→QA/PM证据→选择已有GitHub连接、输入仓库与PR→显式绑定并授予只读取证→查看host读取的账户/数字repo/head→查询此head→查看CI事实及待补执行证明的PM观察→关闭重开仍可查。不是自主QA审查，不触发CI、模型、测试或远端写入。

## Requirements

- R1：host认证GitHub账户、repo读取权限和open PR精确head；前端不提交可信actor/grant。canonical cwd只作本地分区；远端绑定由明确用户动作产生，固定岗位qa_read_pm_observe。
- R2：只GET固定api.github.com，禁重定向，超时/大小/分页限额；前后复核账户/repo/head，配置和凭据变更不沿用旧绑定。无精确head记录明确显示无CI证据。
- R3：项目epoch与绑定、观察独立命名存储。Stop提升epoch，绑定请求携带界面上次读取的expectedEpoch（仅并发条件），来源读取前及最终提交均比对，阻断晚到bind/observe；只有新显式绑定才能继续。最终事务复核epoch；稳定内容去重，观察及PM状态同一记录原子写入。
- R4：app DB迁移兼容、已有记录不改；项目隔离、重开可查，历史绑定保留身份且失效明确。CI成功不等于finding修复或可信runner证明。
- R5：复用现有证据面板与git remote配置；异步结果不跨项目覆盖，Stop不受查询busy禁用。保留本地导入入口，所有状态明确非自治。

## Applicable Harnesses

Spec、Compatibility、Payload、Viewport、AI Collaboration。先后端/前端失败用例、真实SQLite与provider假数据边界回归、独立QA、类型/构建、治理门禁。真实API只读验证与桌面验收分别记录，fixture不冒充生产来源；当前浏览器沙箱阻塞不绕过。

## 状态所有者与兼容

新service仅拥有read-observe绑定及本地观察，不修改S1–S3拒绝态ledger、不把denied升级permit、不接thin/legacy执行状态。新增表与迁移；旧功能查询与工具派发不变。Stop针对本项目只读取证授权，不冒充现有会话Stop或远端CI取消。凭据签名仅本地变更探测，不是执行许可；不持久凭据正文。

## 凭据提交与重启边界

应用内set/delete凭据及git_remotes配置变更共享进程内屏障；最终观察提交在blocking worker持锁驱动DB事务，异步调用者的同步mutation使用try_lock冲突即返回可重试错误，不阻塞Tokio。配置与凭据发生变更（即使改回）使本进程旧绑定失效；所有凭据写入采取保守全局失效。重启生成新nonce，历史可查但再次查询需显式绑定。外部进程直接改系统钥匙串及GitHub远端权限在最后API检查后改变，仍无跨系统原子撤权保证；本片只保存带时间的只读事实，不据此授予执行权。

本地schema只接受当前固定表/索引/版本，未知触发器、索引、部分schema或弱约束拒绝；BEGIN IMMEDIATE保证并发初始化与提交顺序。Stop epoch溢出返回错误并回滚，不伪报成功。与凭据提交冲突的保存/删除应稍后重试，不扩大授权。
