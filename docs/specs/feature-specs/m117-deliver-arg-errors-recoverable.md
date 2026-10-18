# M117：deliver_changes 的参数类拒绝被当成致命错误，会话 2 分钟内恢复耗尽

## 现象（真机 2026-10-18 17:48，v1.89.0 已含 M116 #736）
- 会话 611bcbc9（M115b，PR #738）：deliver_changes 以 through_ci_green 调用，而同一交付已记录 through_merge → 工具拒绝「本次交付上限 through_ci_green 低于同一交付已记录的上限 through_merge；不允许降低交付上限…请沿用已记录的上限继续同一交付」→ 被记为 `agent_loop_error` → 17:50 `technical_recovery_exhausted`。
- agent 本可以按提示重试；实际上这条拒绝没有作为普通工具错误交还给 agent，而是进入了恢复回路并耗尽预算。M116（16:05 74b33b1e）是同类：参数/授权类拒绝直接让会话死掉。

## 需求
- R1 用数据确认：deliver_changes 返回的哪些错误会被归类为 agent_loop_error 并计入恢复预算；写清分类规则。
- R2 参数类/可由 agent 自行修正的拒绝（上限低于已记录、缺字段、正文不合规、场景声明缺失等）一律作为普通工具错误结果返回给 agent，让它在下一步修正，不进入恢复回路、不消耗恢复预算。
- R3 上限低于已记录时，直接沿用已记录的更高上限继续交付（这是同一交付在授权内的既定范围），在工具结果里说明「沿用 through_merge」；只有超出会话授权才拒绝。
- R4 测试（先失败）：以低于已记录的上限调用 → 交付继续且结果说明沿用；构造参数错误 → agent 收到工具错误并能在下一轮成功重试，恢复预算不变。
## 约束
- 不放宽授权边界。合成 fixture，不写生产 DB。

## Implementation Notes

M117 明确替代 M116-R3 对降低调用的拒绝要求；不降低数据库中的既定上限。规格未指定文件名，本文件按已有 M116 命名惯例保存。

### Requirements Traceability / 验收矩阵
| Req | 最低证据 |
| --- | --- |
| R1 | 真实 dispatch/backend/loop 错误边界及合成 fixture；不读取生产 DB |
| R2 | 真工具预检错误返回 is_error，修正后再次调用不返回 fatal；数据库/IO 等系统错误仍传播 |
| R3 | 同 run 的 ceiling 取记录与请求较大值，但不超过当前全局/会话授权；结果说明沿用；不同身份仍拒绝 |
| R4 | 失败优先测试与修改后同命令回归，Rust workspace、前端全量测试和 build |

### Applicable Harnesses
Spec、Compatibility、Observation、Release、AI Collaboration。无 UI/载荷变更，不启用 Viewport/Payload。交付授权仅合并，不发版（not live）。真机验收由编排方发版后完成：已授权 merged 的同一任务，先建立 through_merge 再以 through_ci_green 续接，核对同 run/PR、沿用说明与恢复预算；错误 ceiling/expect_branch 修正后重试；只授权 pr_only 时禁止沿用 through_merge。

### AI Collaboration
context scope：delivery 工具、DesktopToolBackend、agent-loop 错误边界；assumptions：只使用合成 fixture，保留授权和身份门禁；review point：错误不能靠字符串全量吞掉系统故障；validation result：由本任务实际验证输出与 PR 记录补充。任务模板禁止 delegate_tasks，本任务采用单执行流。
