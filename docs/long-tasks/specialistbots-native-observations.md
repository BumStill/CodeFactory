# SpecialistBots 原生只读取证切片

## Basics
- Task ID: CF-SPECIALISTBOTS-NATIVE-S8
- Updated at: 2026-11-05
- Branch: feat/dot-admission-ledger
- Spec: docs/specs/feature-specs/specialistbots-native-observations.md
- Related Req IDs: R1..R5

## Completion Standard
应用内显式项目/岗位绑定、host认证只读GitHub、绑定精确head、持久QA/PM观察与重开；失败测试、独立QA及本地提交，不派发、不推送/合并/部署。桌面验收受阻须保留未通过。

## Current State
已无冲突同步main c3566491，最终44项后端回归通过（新增15、既有DA/DP/DO 29；2个worker入口由父测试执行），74项前端、TypeScript、构建及治理基线/长任务检查通过，独立QA最终无代码阻断。真实PR996 API快照已获取并进入回放测试；不冒称Tauri live。浏览器/桌面仍受阻。

## Completed Items
- 复用现有证据面板、Git remote配置和凭据，新增4个Tauri只读观察命令。
- project canonical分区、固定角色授权、数字repo/账户、精确head、前后来源复核。
- 新schema、原子观察与PM状态、去重与有界历史展示。
- Stop epoch、凭据共享提交屏障、异常schema/并发初始化/溢出修复。
- 显式绑定携带界面读取的epoch，晚到旧IPC不能继承Stop后的新epoch；owned settings guard随提交任务持有，IPC取消不提前解除配置保护。
- 前端绑定、重开、跨项目及Stop竞态测试；真实API快照回放与bounded GET测试。

## Remaining Items
- 桌面与浏览器实地路径验收。
- 可信runner隔离、外部撤权、Stop进程终止和可信执行receipt；本片无自治。

## Blockers
Chromium沙箱配置不符、受限Docker用户命名空间拒绝、本线程无桌面工具；未绕过。外部系统凭据与GitHub权限无法与本地事务原子同步，不能据此开放执行。

## Evidence
详见docs/acceptance/specialistbots-native-observations.md。日志/workspace/s0-evidence/specialist-native-*；快照docs/acceptance/specialist-provider-996.json。第一次测试包装脚本无执行位改用sh；临时隔离core测试离线解析yanked版本后复用仓库锁文件；最终以仓库cargo integration tests为准。

## AI Collaboration
- context scope: 当前工作区、既有Git remote/credentials/DB、独立QA及公开repo只读API；不读取或外发用户任务内容。
- assumptions: provider事实不等于可信执行证明；目录不是远端认证键。
- review point: /root/review_admission逐轮发现凭据竞争/阻塞、schema遗漏、epoch溢出，均先回归后修复；最终独立复核无代码阻断，另独立重跑10项前端通过。
- validation result: 最终组合日志specialist-native-rust-synced.log，前端specialist-native-ui-regression.log；桌面/浏览器未通过不宣称完整验收。

## Stop Boundary
本地提交后交接；不推送、合并、部署。观察命令不会运行测试/模型或写远端；重启/凭据变更后只保留历史读取，查询需显式重新授权。


## 独立交付准备（2026-11-06）
只读S7/S8已从最新主线提取到feat/specialist-readonly-delivery；不依赖执行账本或runner，不应等自治隔离才交付。保留凭据屏障/0027/Stop修复及fixture。独立前端19项、类型与构建通过，Rust专项15个测试入口通过；同步3324d40c后组合复验记录于specialist-readonly-rust-synced.log。验收记录已列AGENTS降级条款、真实门禁状态、S8待人工场景及下一harness工程项。仍not live；当前不推送、不PR、不合并、不部署。

S8跨平台浏览器脚本已补本地可执行入口和合成IPC竞态/重开/失败路径；2项契约先红后绿，未启动真实浏览器。Rust同步后15个测试入口通过。下一步是外部送审授权及新增浏览器target的可信治理注册，非等待自主执行器。
