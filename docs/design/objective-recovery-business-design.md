# Objective Recovery Control Plane：业务设计

## 用户承诺

用户只负责定义目标、授权边界、提供不可替代核心输入，以及决定无安全默认的不可逆业务选项。CodeFactory 负责把等待、provider、CI、远端对账、分支、权限通道、工具失败与进程重启持续推进到目标终态。

## 产品结果

- 用户不再靠“继续”“？”或追问停止原因来充当调度器。
- 相同目标只有一个 objective、一个 change-set 轨迹和一个 canonical delivery identity。
- 阶段性回复、stream 结束和业务完成严格分离。
- 技术恢复耗尽进入产品可观测的 remediation queue，不伪装成用户 blocker。

## 成功指标

- 可避免的用户再提示率下降；recoverable blocker 用户回交率趋近 0。
- 同 objective 重复 PR 为 0；identity mismatch 外部副作用为 0。
- 零输出 transient provider failure 首轮自动恢复；失败时稳定进入 system-owned incident。
- 进程重启后 30 秒内恢复 owner 或给出可审计的 fail-closed 原因。

## 非目标

- 不自动代选不可逆业务结果。
- 不绕过权限、required checks、签名、发布或 live verification。
- 不盲重放已有输出或副作用未知的 tool/provider 请求。
