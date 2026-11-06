# 应用内只读取证与持久观察验收

规格：docs/specs/feature-specs/specialistbots-native-observations.md。

| 需求 | 状态 | 证据 |
|---|---|---|
| R1 认证绑定 | done | src-tauri/tests/specialist_observations.rs::rejects_old_head_account_and_revocation_before_commit；host从连接获取账户/repo/head；真实Tauri连接仍待实地验收 |
| R2 受限来源 | done | src-tauri/tests/specialist_transport.rs::bounded_get_rejects_redirect_oversize_and_does_not_expose_body；snapshot前后复核与精确run来源；不接受任意host/URL/命令 |
| R3 事务/撤权 | done | src-tauri/tests/specialist_observations.rs::concurrent_queries_deduplicate_but_changed_run_gets_new_observation；src-tauri/tests/specialist_credential_fence.rs::mutation_invalidates_old_observation_and_serializes_commit |
| R4 持久兼容 | done | src-tauri/tests/specialist_observations.rs::persists_exact_head_and_deduplicates_on_reopen；migration_is_atomic_and_preserves_existing_app_data；schema异常/epoch溢出回归 |
| R5 真实UI与交互 | deferred | 原因：当前浏览器沙箱及桌面工具阻塞；后续：CF-SPECIALISTBOTS-UI-S8-BROWSER，在正常沙箱/桌面验绑定、原生文件路径、焦点与重开；组件测试 src/components/SpecialistProjectObservations.test.tsx 已覆盖绑定/重开/项目切换/Stop |

## 已观察与尚未证明

真实只读采集：2026-11-05T18:37:32Z，通过已认证gh api GET获得PR996，head5639560f0603fbb97d50d4805ded6adb603a57eb，actor1858168、repo1239349968，5条对应run；前后账户/仓库/PR状态/head一致。随后同步main时确认PR996已由上游合并（版本1.93.0），本快照仅代表采集时刻，不是当前open状态；本次未执行该合并。docs/acceptance/specialist-provider-996.json保留必要字段和采集时间。回放进入生产service解析和真实SQLite，不称为应用内认证transport或桌面live验证。

应用命令已注册到Tauri，使用已有Git remote连接/凭据，固定GitHub.com只读GET；项目键经真实目录canonicalize，远端身份由明确绑定取得。固定角色qa_read_pm_observe。绑定和观察持久保存，重复同状态仅一观察，新run状态形成新观察；按绑定汇总一个本地PM待办，始终needs_execution_provenance。读取界面最多100个绑定、每绑定20个最近观察，旧记录未删除。

既有普通AgentLoop/thin/legacy/拒绝ledger没有被用作执行许可；不启动模型、测试、CI或远端写入。仓库/账户读取权不是执行权。新head不自动继承旧观察。Stop仅针对本项目观察授权，不冒称远端CI已取消；已发GET可在超时前结束，但其晚到结果不得落盘。

## 失败到修复证据

- specialist-native-red.log：缺服务实现；specialist-native-ui-red.log：缺UI实现。
- specialist-native-qa-red.log：schema弱约束与epoch溢出2失败/8通过；补严格schema及有界更新。
- specialist-native-qa2-red.log：同步凭据操作阻塞运行时；改try_lock失败关闭。
- specialist-native-schema2-red.log：未知命名trigger绕过schema检查；改对象name/tbl_name精确前缀过滤。
- 并发schema测试曾报database locked，改BEGIN IMMEDIATE；未改无关旧代码。
- specialist-bind-epoch-red.log：旧绑定请求未携带点击时epoch；加expectedEpoch来源读取前与事务内双复核，Stop后新显式操作才可使用新epoch。
- 后端最终：/workspace/s0-evidence/specialist-native-rust-synced.log（44通过）；前端：specialist-native-ui-regression.log；类型：specialist-native-tsc.log；构建：specialist-native-build.log。

## QA与已完成检查

独立QA最终无代码阻断；另独立重跑10项前端通过；Stop epoch补丁另独立复核4项通过。新增后端15项、前端/Workspace74项、TypeScript与Vite构建通过；既有DA/DP/DO兼容回归另29项通过（2个worker入口由父测试实际启动）；同步main c3566491后的最终组合44项通过，记录在specialist-native-rust-synced.log。配置guard由提交任务持有，单线程runtime测试验证IPC调用取消后也不会提前允许配置写入。

## 限制与交付

not live。本地实现、测试与真实API快照不替代桌面/浏览器验收。凭据屏障只覆盖本进程，不能原子约束外部钥匙串/远端撤权；进程重启保留历史但重新授权才能查询。凭据写入与观察提交临界区冲突会返回可重试错误；所有凭据变更保守失效本进程旧观察绑定。没有可控执行器，因此自主执行继续关闭。

未合并、未推送、未部署。下一步为已授权桌面的真实入口验收，以及受认证执行器的隔离、Stop终止和执行receipt；不是继续扩展导入报告。


## 2026-11-06 独立只读交付候选
本候选分支 feat/specialist-readonly-delivery 从主线 c5e232d0 提取S7/S8，带Stop刷新失败修复、浏览器mock契约修复及报告fixture，不包含S1-S3执行账本、S6采集CLI或S9探针。服务只依赖现有GitHub连接/凭据与SQLite；0027通过ensure_schema接入，不依赖0024/0025 Dot表。凭据屏障和settings/secrets接入不可拆掉。自主执行仍关闭；没有待部署的执行器依赖。

### 真实门禁状态
- 仓库必需：相关失败回归、类型/构建、迁移及权限兼容、独立QA、治理/scenario门禁；独立候选的19项组件/Workspace测试、类型检查、前端构建与脚本契约测试已通过。Rust专项16项在独立候选通过（凭据屏障2、观察13、transport1）；其后同步主线3324d40c，组合复验以specialist-readonly-rust-synced.log为准，不沿用旧head结果。
- 远端必需：agent-bridge-linux、check-frontend、check-rust、governance-baseline、remote-real-app-gui、scenario-gate-pr；本分支未推送，六项尚未执行。CI的Windows headless与精确head场景计划可复用，不必等待本云容器桌面恢复。
- agent live verification not feasible：本任务无可调用桌面/浏览器工具，现Chromium沙箱helper归属异常，未关闭沙箱。此前失败不是只读功能失败；无新条件不重复启动。AGENTS“如果出现新的harness限制”允许明确降级送审，不能豁免必需CI或伪称live通过。
- scenarios needing manual verification：真实已有GitHub凭据绑定open PR精确head→查询→关闭重开历史；Stop与查询竞态；刷新失败后禁用；跨项目切换与旧head拒绝；进程重启需重新授权；宽/窄布局、Escape焦点。已存在对应组件/服务失败测试作为有限替代证据，不能替代完整Tauri链路。
- 立即列为下一工程问题 CF-SPECIALISTBOTS-UI-S8-BROWSER：复用已有headless生命周期工具，使S7脚本跨平台、自启Vite，补S8交互并按治理注册所需target；同时准备真实Tauri凭据/持久化验收。现脚本已补跨平台发现与S8合成交互，但尚未实际运行；UI-009现target尚未注册此脚本，且不包含真实Tauri认证链路；不靠旧CI或窗口截图填补。
- 自主执行隔离、资源限制、executor receipt属于后续自治能力，不是只读切片门禁。真实认证/本地数据安全与迁移仍是本切片必需项。跨平台扩充和完整自治属于增强范围。

### 最短交付路径与授权
本地准备→授权向BumStill/CodeFactory推送feat/specialist-readonly-delivery并开draft PR→仓库自动运行六项必需检查/精确head场景→独立审核降级声明与缺失证据→另获合并权限后按队列严格检查合并。合并不等于发版；依据release-cadence另由授权按需或每日批次发布，并验证tag、资产和updater。合并后可能进入每日批次，申请合并授权时必须说明这一点，不把“不主动dispatch”当永不发布保证。

当前只获本地开发与测试范围，“为啥没上线”“继续验收”不单独等于文件上传、PR、合并或部署授权。最小待授权外部动作是上传此候选分支并开draft PR（将公开代码/证据并触发CI资源使用），不是修改云容器安全配置。不打merge-queue:armed、不主动dispatch发布、不使用未授权Mac。若选择先补真实UI覆盖，仍可本地开发；运行远端新增target须遵守trusted registry/CI修改流程，不能普通PR自行降低或改写裁判规则。


### S8 浏览器验收入口补全
`pnpm test:specialistbots:headless` 已准备：复用allocateLoopbackPort/waitForAcceptanceDocument，发现系统Chrome/Edge（允许CHROMIUM_PATH），自启并有界清理Vite，保持chromiumSandbox=true。375/1280宽度下覆盖S7和S8绑定→查询→关开/页面reload历史、Stop晚到拒绝、来源失败、Stop成功而列表失败、跨项目隔离、主题与截图。IPC和sessionStorage均明确为synthetic；不证明真实GitHub认证、SQLite或应用进程重启。

新增Node vm契约先失败（缺bind），实现后2项通过；独立QA发现晚到请求未等settled，新增断言先失败再修正，最终2项通过、node --check通过。真实浏览器未执行，不记录DOM通过；当前云沙箱前提未变化。既有权限模式headless CI可运行真实Chrome/Edge，但不会自动调用新命令。

新增target的最小治理变更是给UI-009 automated_by增加pnpm:test:specialistbots:headless；这是TRUST_ROOT_FILES保护的scenario-registry，需由受信治理更新流程审查/落地，不能由普通功能PR自认证。已在本地准备建议patch，不改可信根。也可在已授权桌面直接运行同一命令取得补充证据；不为此开放未授权Mac。六项required checks依旧全部必需。

最终组合验证：主线3324d40c+独立切片的Rust16项全部通过，记录specialist-readonly-rust-synced.log；本次未改前端产品源码，19项UI、tsc、Vite构建证据仍适用于该前端树。完整全量/跨平台required CI尚未执行。构建生成的schema仅留本地证据，不纳入产品提交。
