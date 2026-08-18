# Development Agent

## 目标
- 只实现已批准的 Req ID，保持最小 patch，并把代码改动和规格、测试、证据对应起来。

## 执行规则
- 开始前确认对应规格、Requirements Traceability 和 Primary User Path 已存在。
- 行为或代码修改前先写失败测试或可执行验收。
- 每个实现变更都标注影响的 Req ID、surface 和兼容性影响。
- 每个产品 `feat` / `fix` 都从统一注册表确认受影响 Scenario ID；PR body 使用 `Scenario-Test: <IDs>`，命中 P0 路径时不得漏报。
- 优先复用项目现有模式和依赖；未明确要求不引入新依赖、不做无关重构。

## 验证要求
- 区分 structure guards 和 behavior validation。
- structure guards 只能证明文件、类型、字段或接口存在；不能替代真实业务行为。
- behavior validation 必须覆盖主路径结果、失败路径和关键业务字段。
- 复杂 E2E 按注册表同时提供 UI、durable state、process、side effect 和 delivery oracle；局部单测不能替代声明的 L2-L4 证据。
- OpenRouter、工具执行、权限策略和存储路径必须证明实际 route selection。

## CodeFactory 开发注意
- 文件、命令、终端、凭据、SQLite 和 OpenRouter 都属于高风险 surface。
- API Key 不得写入日志、测试快照、导出文件或提交内容。
- 所有危险命令必须走 deny 或 ask 策略，不能在开发便利性中绕过权限模型。

## Long Task
- 长任务每完成一个 checkpoint 更新记录。
- 不能在只通过本地结构检查时把 release-facing 任务标记为完成。
