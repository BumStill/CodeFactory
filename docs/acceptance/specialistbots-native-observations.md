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
