## M198：Python heredoc 安全证明与可审计审批理由

Scenario-Test: UI-009
README-Update: reviewed
README-Update-Reason: 修复既有 trusted 权限判定及审批诊断，不改变公开安装、平台或隐私承诺。

## 根因
`trusted_git::executable_segments` 按 shell 引号与换行拆分解释器正文。Python 三引号正文中英文撇号会留下未闭合 shell 引号，`sensitive` fail-closed 后进入无法证明的 Git 备份路径，最终弹审批；相反，解释器 heredoc 内子进程调用可能未命中原分段规则。新增完整 quoted Python heredoc + AST 白名单证明，真实登记工作区路径及符号链接边界独立验证。

## Req ID / 验收
- R1（部分）：Python 本地静态文件替换通过；越界、凭据、子进程、网络、动态路径和未知语法保留审批。Node 等其他解释器暂不放行，见 Spec Amendment。
- R2：后端覆盖模型伪造的 `_permission`，rule/reason 落库于现有 `prompt_args_json`，事件、恢复和后台 pending approvals 沿用同一数据；诊断字段不改变可执行动作签名。审批卡片独立显示理由与规则，旧记录明确标记缺理由。
- R3（部分）：`m198_quoted_python_is_not_shell_syntax` 与 `m198_heredoc_danger_is_never_treated_as_inert_data` 首轮均失败，修复后 9 项解析回归通过。`M198 renders the persisted interception reason and rule` 首轮失败，修复后 11 项卡片测试通过。原始任务 JSON 有截断标记，尚未获得未截断原文 fixture。
- R4（合成等价命令）：`m159_gateway_session_identity_backup_and_remote_fence` 在真实 trusted 网关、身份验证后的合成工作区执行 Python 修改，断言文件内容、审批意图数为 0；越界对照不执行且外部文件不变，并断言落库规则/理由及授权签名一致。该测试已通过。
- 生成/安全反例：`node scripts/verify-m198-python-ast.mjs` 通过，19 个变量输入、本地 helper、禁止执行被测正文、越界/凭据/网络/子进程/动态路径/符号链接。
- 真实浏览器：`node scripts/verify-m198-permission-headless.mjs` 通过，light/dark × 1366/390，理由、规则、按钮均在视口内。截图 `/tmp/codefactory-m198-permission/reason-{light,dark}-{1366,390}.png`。
- 全量 `pnpm test` / `pnpm build` / Rust workspace 测试：后台执行中，结果待更新，不能视为通过。

## Applicable Harnesses
Spec：版本化规格及 R1–R4 映射；Compatibility：沿用 prompt JSON，无 schema 迁移，签名排除诊断元数据；Payload：解析完整 heredoc，AST 不执行正文，边界与危险反例；Observation：可查询落库 reason/rule；Viewport：真实浏览器深浅色与窄屏；Release：本次只合并，不发版，not live；AI Collaboration：下述记录。

## AI Collaboration
Context scope：可信 shell、登记工作区、审批意图与卡片；Assumptions：仅自动允许可静态证明的 Python 文件编辑，不把任意程序称为安全；Review point：越界、子进程、凭据、未知语法、签名与元数据伪造；Validation result：针对性测试与真实浏览器通过，全量结果待更新。按任务模板不调用 delegate_tasks，当前单执行流，未启动多角色子代理。

## Spec Amendment（待编排方决策，不视为已批准）
1. 原始 JSON 在 classify_extension_error 处包含 `…<truncated>`，缺少脚本尾部与 heredoc 终止符；无法按原文执行 R3/R4。建议提供未截断 fixture 后补精确回放；当前使用完整合成同形命令，不宣称已执行事故原文。
2. 当前实现仅支持 Python 的静态文件编辑子集，Node 与任意解释器程序仍 fail-closed。不能满足 R1 对所有解释器的宽泛要求；建议单独定义各语言可证明子集并补实现，不能无条件信任未知脚本。

## 真实应用验收分工
真机验收由编排方发版后完成：安装含此提交版本，trusted 会话在已登记工作区执行 Python heredoc 文件替换，确认无审批且 diff 正确；越界、git push、网络对照仍要求确认；核对 `permission_intents.prompt_args_json._permission` 与卡片一致。不改生产数据库。本 PR 未发版、not live。

Release-Urgency: immediate
