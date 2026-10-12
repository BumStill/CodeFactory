# Windows prefer-all-down 测试前置条件修复

## 背景与决定
用户报告 2026-10-12 #635 Windows check-rust 偶发失败：`prefer_mode_waits_only_after_every_endpoint_is_unavailable` 期望系统继续观察恢复，实际得到 `PROVIDER_RECOVERY_WAITING: provider overloaded`。仅修复测试真实前置条件，不扩大超时、不增加重试、不改变生产恢复策略；当前受管分支基于最新 main，授权 PR 进入合并队列，不发版。

## Requirements Traceability
| Req ID | 要求 | 最小证据 |
| --- | --- | --- |
| CF-PFB-R2 | 三端点均不可用时逐一尝试并给出平实等待状态 | 原测试保持文案及端点顺序断言，追加三条 failed_replayable/provider_rejected 账本断言 |
| CF-PFB-R7 | 测试不依赖 CPU 调度或端点状态竞争 | 新 fixture 回归先红后绿；CPU 压力下原测试 100 次及 model_transport 全套 20 次 |
| CF-PFB-R8 | 保留生产共享端点健康语义、测试隔离 | 生产逻辑不变，沿用实例级 EndpointHealthRegistry；fixture 持有端口 |

## 根因与实现约束
`unreachable_base_url` 的服务线程 accept 后未读请求即写 502 并关闭。未消费的 POST 可使 TCP 连接重置，客户端因此得到含随机 loopback 端口的传输错误，而非显式 HTTP 502。`is_provider_overloaded` 使用子串 429/503/529 分类；传输错误 URL 的随机端口可能命中这些数字，最后一次失败进入三次过载预算的 DurableWaiting 分支。因此表现为文案分支改变，但源头是 fixture 的协议时序，不是共享端点健康竞争。实例级健康注册表已隔离；不修改生产分类器的既有语义。

fixture 必须读完 Content-Length 对应的 POST 再发送 502；不使用 sleep、加长超时或额外重试。增加 1 MiB POST 的专用回归：旧 fixture 本地报 reqwest UnexpectedMessage，修复后断言 HTTP 502。原测试同时检查真实 SQLite failure_class/status，确保前置条件不再默许传输错误。

## Applicable Harnesses
Spec Harness：本文件及 Req ID 映射。
Compatibility Harness：生产 API/策略无改动，现有 model_transport 测试回归。
Observation Harness：按端点顺序及 SQLite 状态/分类字段核验。
AI Collaboration Harness：仅当前任务上下文；假设 502 fixture 必须真正交付 HTTP 响应；审查 TCP 请求消费和持久化分类边界；验证先红后绿及压力循环。按用户要求不启用 sub-agent。

## 验收矩阵
- 大 POST 请求 → 显式 502，而非连接重置：独立回归先失败后通过。
- 三端点 → 顺序 chatgpt/deepseek/openrouter，三条 failed_replayable/provider_rejected，保留平实等待文案。
- CPU 压力 → 原测试 100/100；model_transport 全套 20/20。
- 全 Rust workspace、前端测试/build、场景与 README PR 正文 validator。
- 无 UI 变更：真实浏览器/截图不适用；本任务只改测试，不能据此宣称生产安装包 live 验证。
