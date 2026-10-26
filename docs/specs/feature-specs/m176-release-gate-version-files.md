# M176（紧急，阻断发版）：release 场景门禁因版本号文件「未映射」失败

## 证据
Release v1.91.36（run 40283384368）prepare-release / scenario-gate-release：
`##[error]scenario registry coverage gap for product files: package.json, src-tauri/Cargo.lock, src-tauri/Cargo.toml, src-tauri/tauri.conf.json`
v1.91.35（10-26 13:4x）同一流程通过；其间合并的有 #872（README）；M165（#857，scenario-mappings.json 只增映射）10-24 合并后这是第一次需要新 patch 版本号以外的检查？请查清回归来源（可能是 M165 合并映射/候选树逻辑在 release 阶段未读到 registry 中原有的版本文件映射，或 bump 提交被当成产品改动）。
## 需求
- R1 定位根因并修复：版本号 bump（chore: bump version）只改这 4 个文件时，release 场景门禁应按既有规则通过（这些文件属发布元数据，应有明确映射或豁免，且豁免在信任根里定义、不能被候选 PR 自改）。
- R2 回归测试：release 阶段对「仅版本号 bump」的范围 → 通过；真实产品文件未映射 → 仍失败。
- R3 修复合并后让 v1.91.36 的内容（含 #872 等）正常切版（失败标签不会自动重切，按 docs 用新 patch 号）。PR 标题前缀 fix(release):。
## 约束
- task-template；授权交付（ceiling: merged）；改 tools/governance 或 registry 属信任根，PR 写明需要 gate-dance。

Release-Urgency: immediate

## Implementation Notes

- 根因：`scenario_impact_files` 以整个发布范围文件集等于四清单为前置条件。v1.91.35..v1.91.36 含 README/文档变更，使同步版本豁免未执行。随后 release 禁止全局展开、又无清单显式场景，触发兜底 coverage gap。M165 的追加映射不是根因。
- 最小修复：仅对四清单的限定 diff 使用原有严格同步版本判定；满足时从影响集合移除四清单，保留其他文件。非版本行、缺文件、不一致版本、git 错误均保持 fail closed。不改映射、全局展开或 trust-root immutability。
- Requirements Traceability：R1 → 混合文档/已映射产品的版本豁免；R2 → 纯 bump、未映射产品、配置修改及不同版本拒绝；R3 → immediate trailer 交由队列合并后显式 dispatch 新 patch，保留失败 tag。
- Applicable Harnesses：Spec、Compatibility、Release、Observation、AI Collaboration。测试使用临时真实 Git 仓库与合成版本数据，执行真实影响计算和 release 覆盖判定；无 UI 变更，不适用 Viewport。
- 交付上限 merged；信任根修改预期需编排方 gate-dance，禁止自行削弱规则。真实安装版验收由编排方发版后完成：确认新 patch 安装/启动、版本一致、README 所述功能仍可用。
