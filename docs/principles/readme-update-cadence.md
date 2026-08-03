# README 更新机制

配套设计：[业务设计](../design/readme-update-mechanism-business-design.md) ·
[架构设计](../design/readme-update-mechanism-architecture-design.md) ·
[体验设计](../design/readme-update-mechanism-ux-design.md)

## 目的

`README.md` 是面向用户的稳定产品契约，不是每次发版的变更日志。过去 README
只在少数提交中被顺手修改，导致新能力、安装路径和发布流程逐渐与真实产品脱节。
本机制把“要不要改 README”从个人记忆变成可审计的 PR 决策，并让静态漂移在合并前失败。

## 规则

每个 PR body 必须出现且只能出现一组机器可读字段：

```text
README-Update: required|reviewed
README-Update-Reason: <why this decision is correct>
```

| 决策 | 适用情况 | 门禁 |
|---|---|---|
| `required` | 新增或改变用户可见能力、平台/provider、安装/更新、安全/隐私、兼容性、公开产品承诺或 roadmap 状态 | 同一个 PR 必须修改 `README.md` |
| `reviewed` | 仅内部重构、测试、CI、依赖、治理文档或不改变用户产品契约的变更 | 可以不改 README，但必须写清理由 |

以下内容不写进 README：某次 release 的精确版本、完整变更日志、临时测试结果和
只对开发者有意义的内部实现细节。它们分别进入 GitHub Release notes、测试/证据和
`DEVELOPMENT.md`/设计文档。README 使用 `releases/latest` 等版本中立链接。

## 执行与节奏

1. PR 模板提供默认 `reviewed` 字段，作者必须按实际影响改成 `required` 或保留
   `reviewed` 并补充理由。
2. CI 的 `README contract` 检查每个 push/PR：章节、维护标记、相对链接和版本中立性
   必须通过；PR 声明 `required` 时，README diff 缺失直接失败。
3. 自动版本 bump PR 显式声明 `reviewed`，因为版本文件变化不应触发 README 噪音。
4. 发布流程只生成 Release notes，不自动重写 README；发布不会成为 README 的隐式
   触发器。
5. 每月第一个 UTC 日创建一条 `[README review] YYYY-MM` 复核 issue。owner 对照已发布
   app、Install/Quick start、Features、Data & privacy 和 Roadmap；需要时开普通 PR
   更新，确认无变化也要在 issue 中留下结论。issue 创建幂等，不自动编辑正文。

## 责任与失败恢复

- 功能 PR 作者：判断用户契约影响并在同一 PR 更新 README 或说明 `reviewed`。
- Product/Release owner：处理月度复核，维护公开能力和安装承诺的准确性。
- CI/治理维护者：保持 validator、PR 模板和自动版本 PR 的字段一致。

缺字段、重复字段、占位理由、README 缺章节/坏链接/硬编码版本时，先修 PR 再合并；
不要通过删掉检查或让 bot 直接改 README 来“恢复”流水线。若发现发布后漏写，开一个
`README-Update: required` 的补充 PR，并在月度 issue 记录原因。

## 验收指标

- 100% 进入主分支的 PR 有唯一 README 决策和理由。
- `required` PR 中 100% 同时包含 README diff。
- 每月最多一条复核 issue，且不会自动改写 README。
- README 不出现精确 release 版本；下载入口始终指向 latest release。
