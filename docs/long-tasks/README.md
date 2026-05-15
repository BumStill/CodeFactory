# Long Tasks

本目录记录跨阶段、跨角色或 release-facing 的治理任务。

## 何时创建
- 需要规划、开发、QA、发布多个角色交接。
- 需要多次验证或多轮发布。
- 当前轮无法完成，但必须保留明确完成边界和 blocker。

## 停止条件
- 完成：所有 Req ID、主路径、验证和证据包达标。
- 阻塞：记录 blocker、证据、下一步命令或人工动作。

## 验证
- 使用 `python tools/governance/validate_long_task_record.py --task-record-path <path>` 检查记录结构。
