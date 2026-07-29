# 通用代码交付、部署与线上验证

状态：实施中  
Owner：agent delivery  
适用 Harness：Spec、Compatibility、Release、Observation、AI Collaboration

## 问题

`deliver_changes` 不能把未知 Git 托管平台误报为“缺 GitHub CLI”，也不能把 PR/MR 合并、发布工作流触发或 PaaS 部署启动表述为“已经上线”。GitHub Enterprise、GitLab Self-managed、Bitbucket、Azure DevOps、Gitea/Forgejo、Gerrit、CodeCommit、普通企业 Git，以及 Zeabur/Vercel/Netlify/Render/Railway/Kubernetes 等外部部署系统必须能通过统一能力模型被正确识别、接入或明确阻断。

## Requirements Traceability

- **REQ-DEL-001 Provider-aware discovery**：从实际 push remote 的 URL 识别 forge family 与 host；GitHub CLI 认证必须按该 host 探测。未知/非 GitHub remote 不得提示 `gh auth login` 是通用修复。
- **REQ-DEL-002 Remote-neutral local delivery**：默认分支、ahead 判断和 push 使用解析出的 remote，不硬编码 `origin`；没有 review adapter 时允许完成 commit/push，但 PR/MR 阶段必须给出 provider-aware blocker。
- **REQ-DEL-003 Layered states**：结果必须区分 `release_triggered`、`deployment_pending/succeeded`、`live_verified`；只有 live verifier 成功后才可使用“已上线/上线已验证”。
- **REQ-DEL-004 Hook adapters**：现有 `delivery_provider` JSON hook 除 PR/MR、CI、merge、release 外，支持 `deployment_status` 与 `verify_live`，用于 Zeabur 和企业 CD。
- **REQ-DEL-005 Repository-owned live checks**：仓库可提交 `.codefactory/delivery.json`，配置无密钥 HTTP live assertion；支持状态码、响应正文包含值，以及用 `$GIT_SHA`/`$GIT_SHA_SHORT` 绑定本次交付版本。
- **REQ-DEL-006 Fail closed**：未配置部署观察或 live verifier、部署仍 pending、线上断言不匹配时，不得返回 `delivered` 或声称上线；应返回可恢复的 `blocked` 并保留已完成步骤。
- **REQ-DEL-007 Backward compatibility**：旧 Settings、旧 hook 和没有 `.codefactory/delivery.json` 的仓库继续可读；旧 hook 对新 action 返回 unsupported 时显示“未配置上线验证”，不得崩溃或伪造成功。
- **REQ-DEL-008 Verification phase boundary**：重型验证（完整测试、构建、治理、主路径验收）必须在 merge/release 前完成；发布后只做轻量事实确认与 live smoke，不得把发布后的重复全量测试当作常规完成条件。发布后只有在 pre-release 证据缺失/过期、release workflow 在发布阶段生成或修改了需重新验证的代码/制品逻辑，或 release/live smoke 失败时，才扩大到针对性重验。

## Repository-owned configuration

`.codefactory/delivery.json`：

```json
{
  "schema_version": 1,
  "remote": "origin",
  "provider": "zeabur",
  "deployment_timeout_secs": 900,
  "live": {
    "url": "https://example.com/health",
    "method": "GET",
    "expected_status": 200,
    "body_contains": "$GIT_SHA_SHORT",
    "timeout_secs": 300,
    "poll_interval_secs": 10
  }
}
```

配置文件不得保存 token。Zeabur deployment status 等需认证的 API 通过 Settings 中现有 `delivery_provider` RunCommand hook 接入；hook 从其进程环境/本机凭据读取密钥。

## Hook protocol additions

输入继续为单个 JSON：

- `{"action":"deployment_status","sha":"...","provider":"zeabur"}`
- `{"action":"verify_live","sha":"...","url":"..."}`

返回：

- `{"status":"success","detail":"Zeabur production deployment ready"}`
- `{"status":"pending","detail":"building"}`
- `{"status":"failure","detail":"build failed"}`
- `{"status":"unsupported","detail":"configure deployment observer"}`

`verify_live` 的 `success` 只能表示 hook 已对真实服务执行了字段/行为断言；单纯 HTTP 200 不满足本规格。

## Primary User Path

1. Agent 完成改动并在发布前验证本地行为：相关测试、构建、治理与主路径验收必须晚于最后一次源码/配置修改。
2. `deliver_changes` 解析实际 remote、提交并推送。
3. provider adapter 打开 PR/MR/Change，等待 CI 并合并；CI 是发布前质量门禁。
4. release action 只记录“已触发”，不记录“已上线”。
5. 发布后只执行轻量事实确认：tag/commit 归属、release 非 draft、资产存在、latest/updater 指针、release workflow 结论。
6. deployment observer 等待 Zeabur/其他 CD 得到成功结论。
7. live verifier 对真实 URL 执行绑定本次 commit 的断言。
8. 只有第 7 步通过，结果才显示 `live_verified` 和“上线已验证”。发布后不得常规重跑全量测试；那类验证应前置到第 1–3 步。

## Verification boundary

- 发布前：运行完整或相关的测试、构建、类型检查、治理检查、主路径验收，并等待 PR/CI 绿灯。
- 发布后：只验证发布事实与用户可见入口，包括 release workflow 结论、tag 包含 merge commit、release 已发布且资产齐全、latest/updater 指向新版本、配置的 deployment/live smoke 通过。
- 禁止模式：GitHub release 已发布后，为了“证明发布”再无条件重跑全量 Rust/前端/build/headless；这些只能证明源码当前还能过，不能改变已发布资产，且会浪费用户时间。
- 升级条件：若发布脚本在 release 阶段生成了新代码、资产签名/打包逻辑改变、pre-release 证据缺失或 live smoke 失败，才运行最小相关重验。

## Testing Matrix

- GitHub.com、GitHub Enterprise、GitLab、Bitbucket、Azure DevOps、Gitea/Forgejo、Gerrit、CodeCommit、generic SSH URL 识别。
- 非 `origin` remote 的默认分支解析、ahead 检查和 push。
- 旧 hook 对新增 action 不支持的兼容路径。
- deployment pending → success；failure；timeout。
- HTTP live verifier 成功、状态码失败、body/SHA 不匹配、网络失败。
- 未配置 live verifier 时 release 只能是 triggered/unverified，禁止“已上线”。

## Assumptions / Review point / Validation result

- Context scope：交互式 `deliver_changes`、Settings hook、workspace delivery status；不改变普通手工 Git 命令。
- Assumption：需要密钥的平台 API 不把 token 写入仓库，统一由 hook 的安全环境提供。
- Review point：任何把 deploy/release success 自动升级为 live 的路径都必须拒绝。
- Validation result：待实现后回填 Rust focused tests、前端测试、build、真实 hook/HTTP smoke、PR/CI/发布证据。
