# M220（由 M218 去掉合盖部分改写）：唯一可用的端点出一次网络错误，会话就被挂到别的端点重置（停两天半）

## 现场（会话 c39b838e，M217）
- 2026-11-01 07:58（周日，DeepSeek 全天可用）这一轮的路由尝试：
  - chatgpt 被 skipped：subscription_quota_cap，11-04 14:44 才重置。
  - openrouter 失败：CREDENTIAL_UNAVAILABLE（没配置凭据）。
  - deepseek 失败：TRANSPORT_UNREACHABLE「HTTP error: error decoding response body」，只这一次，属于瞬时错误。
- 随后系统挂了「当前没有可用模型：ChatGPT 已达订阅用量上限，11-04 14:44 重置」的唤醒，会话从周日早上一直停到周三下午。在此期间 DeepSeek 一直是好的，有大量可用的闲时窗口。
- 界面上给出的理由（ChatGPT 额度）和真实原因（DeepSeek 一次网络抖动）对不上，用户会被误导。

## 补充（11-03 23:08 M217 真机验证时复现，与合盖无关）
- DeepSeek 因 daily_spend_cap 被跳过（0 点重置），ChatGPT 因订阅额度被跳过（00:51 重置），OpenRouter 没有凭据。会话被挂到 00:51，而不是最早可恢复的 00:00；等待原因只提到 ChatGPT，没提 DeepSeek 已到每日上限。
- 用户决定合盖造成的停顿不处理，原 R0/R5 删除。

## 要求
- R1：「没有可用模型」时的唤醒时间，必须取所有候选端点里最早能恢复的那个。瞬时错误（TRANSPORT_*、5xx、超时、读响应失败）属于马上可以重试，按指数退避处理（例如 1、2、5、10、30 分钟，之后每 30 分钟一次），不能拿另一个端点的额度重置时间来定唤醒。凭据缺失的端点不参与计算。用量窗口不允许用的端点，按下一个允许时段的开始时间算。
- R2：等待原因要写真实的那一条：例如「DeepSeek 连接出错，N 分钟后重试」，而不是挑一个不相关端点的理由。
- R3：同一个端点连续出现瞬时错误超过上限（例如 2 小时内 12 次）时，才升级为明确的失败提示，并交代试了什么。不许出现无限静默等待。
- R4：测试覆盖两个组合：(a) chatgpt 额度封顶 + deepseek daily_spend_cap → 唤醒取较早的重置时间，原因同时写两者；(b) 本次组合：chatgpt 额度封顶 + openrouter 无凭据 + deepseek 瞬时错误。断言唤醒时间 ≤ 2 分钟后，等待原因提到 DeepSeek。另外覆盖「deepseek 在用量窗口外」的情况：唤醒时间等于下一个允许时段的开始。

## 验收
- 真机：把 DeepSeek 每日上限临时调到低于已花金额，并让 ChatGPT 超额，会话应挂到 DeepSeek 的 0 点（两个重置时间中较早的那个），等待原因同时写出两个端点的真实原因。
- 真机上无法人为制造网络错误时，用测试替身端点（返回无法解析的响应体）在应用里跑一轮。验收证据：会话在几分钟内重试，界面文案正确。

Release-Urgency: immediate

## Implementation Notes

- 保持 R1–R4 不变；不处理合盖。现有 `ActiveRouteState`、`ModelUnavailable`、SQLite 模型等待与 thin/legacy 结算复用同一恢复信息，不改配置与凭据协议。
- Spec / Compatibility / Observation / Release / AI Collaboration Harness：以真实路由准入、耗尽计算、SQLite 持久等待和唤醒集成测试提供字段级证据；完整前端测试和构建做回归。
- 真机验收由编排方发版安装后完成：按上文两项验收核对实际等待原因、重试时间与审计记录。本任务交付上限为合并，不发布、不宣称 live。
- Req→测试：R1/R2/R4 覆盖混合额度、预算、凭据、瞬时错误和用量窗口；R3 覆盖端点级连续退避、达到上限终止、成功清零与恢复持久性。
- AI Collaboration：context scope 为上述恢复路径；assumption 为既有路由失败在无副作用前可重放；review point 为候选内部取最晚、候选之间取最早及重试上限；validation result 记录于 PR。按用户模板禁止 `delegate_tasks`，采用单执行流。
