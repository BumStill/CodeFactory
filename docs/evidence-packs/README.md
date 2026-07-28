# Evidence Packs

本目录保存 release-facing、重大特性、主路径验收和治理变更的证据包。

## 命名
- `YYYY-MM-DD-<feature-or-release>.md`

## 最低内容
- Primary User Path 证据。
- 字段级断言。
- 实际处理 route。
- 失败路径或回滚边界。
- AI Collaboration 记录：context scope、assumptions、review point、validation result。

## 禁止作为唯一证据
- UI 出现。
- HTTP `200`。
- mock 成功。
- 非空数组。
- 行数正确。
- deploy 命令成功。

## 保留策略

本目录只保存需要长期留存的证据:release-facing 证据、`*-locked-*` 基线。

**不要**把一次性迭代/回归子集快照(文件名含 `T\d{2}-\d{2}-\d{2}Z` 这种时间戳后缀,例如
`terminal-bench-21-iteration-2026-06-29T12-07-06Z.md`)提交进本目录——这类快照应该留在
本地 `.codefactory/benchmark-jobs/` 或作为 release asset,不进仓库。CI 的
`evidence-pack-retention` 治理规则(`docs/governance/rules.yml`)会在 PR 里对新增的此类
文件发出警告。2026-07-28 一次性清理了 59 份历史遗留的迭代快照。
