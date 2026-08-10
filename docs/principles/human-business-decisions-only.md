# 人只决策关键业务判断，系统不得回交非业务阻断

## Principle

CodeFactory 可以请求人对关键业务判断作决定，但默认仍应采用系统推荐方案继续完成；不得把任何技术、执行、工具、环境或恢复问题包装成人工门禁。

当用户明确要求“搞定”“全部完成”“不要等我”或表示将离开时，当前 objective 进入 `autonomous_completion=true`：在既有授权范围内自动采用推荐配置、完成实现、交付、发布和验证，不等待普通决策确认。

## 合法的人类决策

只有当选择会改变以下任一业务结果、系统没有可接受的推荐默认、且选择不可安全撤销时，才允许进入 `needs_business_decision`：

- 产品范围、用户可见行为或验收口径；
- 不可逆数据语义、数据所有权或删除边界；
- 成本、发布时间、市场承诺或质量风险之间的真实取舍；
- 法律、合规、安全权限扩大或凭据授权边界；
- 多个都可执行但业务后果不同的方案选择。

只要存在明确推荐且动作可逆、风险已在用户授权范围内，系统直接执行推荐项。请求必须包含 `decision_key`、互斥选项、推荐项、各自业务影响、为何系统不能代选以及安全默认动作。没有这些字段的请求不得回交给人。

## 核心外部输入例外

无法推导、无法自动获取且由外部主体控制的核心输入（例如首次身份凭据、法律主体确认、不可替代的生产账号授权）可以请求人提供，但必须同时满足：

1. 系统已尝试现有凭据、受管身份、等价官方通道、自动刷新和安全恢复路径；
2. 把同一 objective 的全部缺失输入合并为一次请求，禁止零碎频繁打断；
3. 请求说明已尝试路径、仍缺的最小输入、输入后的自动续接点；
4. objective 保持未完成并自动等待输入，不要求用户再次描述任务；
5. 不得通过跳过签名、测试、发布、live verification 或缩小功能来降低用户要求。

## 禁止回交的非业务阻断

以下状态始终由系统拥有，不得要求用户回复“继续”、重新授权同一目标或替系统排障：

- CI/test/build/lint 失败；
- merge queue、required checks、branch behind、冲突与重复 PR；
- 网络超时、限流、远端暂不可达、provider/model 单点失败；
- 工具 timeout、进程崩溃、App 重启、锁和残留进程；
- 依赖缺失、工作目录错位、缓存或构建环境问题；
- context/token budget、会话压缩、模型输出不完整；
- 已获授权范围内的常规权限、凭据刷新和交付续接；
- Agent 恢复预算耗尽。

系统必须自动诊断、修复、退避、切换等价执行路径或进入后台 incident/remediation。无法自动解决时写 `failed_internal` 或 `platform_incident`，保留 objective 和证据，触发系统维护路径；不能写 `needs_user`。

## Decision router

```text
blocker observed
  -> can system repair/retry/reconcile/switch route? -> system_owned_recovery
  -> does a safe default preserve the user's objective? -> apply safe default
  -> is there a recommended reversible option within authority? -> apply_recommended
  -> is an externally controlled core input truly unavailable? -> batched_core_input_request
  -> do irreversible options change material business outcome with no safe default? -> needs_business_decision
  -> otherwise -> failed_internal/platform_incident + remediation queue
```

## KPI

- 非业务阻断用户回交率：目标 0%。
- 已授权 next action 要求重复确认：目标 0。
- `needs_business_decision` 精确率：100%，每条均有结构化业务影响。
- 推荐配置自动采用率：目标 100%，已声明无人值守 objective 不等待普通决策。
- 核心输入请求次数：同一 objective 最多一次合并请求。
- 单模型、单工具、单进程失败后的自动恢复成功率与恢复时间必须可观测。
