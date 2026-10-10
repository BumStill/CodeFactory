# CF-LEASE 自验收与交付

实现范围：CF-LEASE-R1～R5；当前受管分支，基于最新 main；ceiling: pr_only。

## 自验收表
| Req | 结果 | 证据 |
|---|---|---|
| R1 | 通过（SQLite 集成） | cf_lease_release_all_outcomes_and_reclaim_are_audited：running/waiting/failed/completed 后释放；非终态立即再次准入。foreground guard 正常返回显式释放，取消 Drop 异步清理，TTL 兜底。 |
| R2 | 通过（有界 TTL） | 上述测试模拟过期残留并断言 lease_takeover 审计；释放审计与 epoch fencing 不变。 |
| R3 | 通过（注入重试 + 编译接线） | cf_lease_network_retry_budget_and_proxy_config：fetch/push/pr view/pr create 瞬时失败第 3 次成功，持续失败 3 次停止；1/2 秒退避。PR create 重试前精确查询，未知不创建。 |
| R4 | 通过（真实 git 子进程） | 合成 GIT_CONFIG_GLOBAL 中 http.proxy 读取等于 fixture；dev_command 不清空继承环境，HTTPS_PROXY 保留。未改写用户配置。 |
| R5 | 通过 | cf_lease_stalled_report_names_owner_and_preserved_state；30 秒子进程 deadline；输出阶段、等待动作、责任人、提交/分支/PR 证据。 |

TDD：SSL 分类断言失败后修复；租约释放 API 缺失编译失败后实现；重试 API 缺失编译失败后实现；等待报告阶段断言失败后修复。未降低已有测试期望。

## 验证
- 专用 CARGO_TARGET_DIR cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：通过。
- pnpm test：136 文件、911 测试通过（含 lightModeAudit）。
- pnpm build：通过。
- 治理基线：通过。
- 新增测试均通过；真实桌面跨重启与发布安装包仍需在发布验收执行，not live。本次只授权开 PR，不合并、不发布、不执行合并后 §8 远端分支清理。

## Harness
Spec：原样规格；Compatibility：无 schema 改动、owner/epoch CAS；Observation：lease_release/lease_takeover 事务审计；Release：PR-only，保留门禁；AI Collaboration：单执行流（用户禁止 delegate_tasks）。

AI Collaboration：context scope 为 tools/delivery、agent/delivery、delivery_run；assumptions 为环境继承和有界 TTL 回收；review point 为模糊远端结果禁止盲重放；validation result 为完整 Rust、前端测试及 build 通过。无 frontend 布局变更，不适用 Viewport 截图。

README-Update: not-needed
README-Update-Reason: 内部交付锁与网络恢复修复，不改变公开产品或安装承诺。

Scenario-Test: HLT-001, HLT-002, HLT-005, CXD-002, E2E-001, E2E-002, E2E-003, E2E-004, E2E-005

Spec Amendment: 无。
