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
