# M224 安全边界审查与 Spec Amendment（待裁决）

## 状态

实施阻塞，未改产品代码、未提交、未开 PR、未武装合并队列。规格要求安全边界有错或做不到时停止，不得自行放宽。本记录不修改 R1–R10。

## Spec Amendment

### 问题：R3 的可写工作区与不可写应用数据目录重叠

当前受管工作区通过 `Path.resolve()` 验证位于：

`~/Library/Application Support/com.codefactory.app/execution-workspaces/<repo>/<objective>`

R3 同时要求该登记工作区可写、应用数据目录不可写。对应用数据目录使用 blanket `deny file-write* (subpath ...)` 时，其后代工作区即便单独 allow 仍不可写。

### 建议（尚未实施，需编排方裁决）

将 R3 明确为：应用数据目录除本会话经身份验证、已登记的工作区外不可写；工作区例外不包含 settings.json、数据库、凭据、会话日志及其他会话工作区。内部临时日志由宿主进程写，不向命令开放应用数据目录。

另一选择是将 execution-workspaces 移出应用数据目录；这涉及工作区分配、旧登记兼容与迁移，不应在本单擅自扩展。

### 原因

不能在实现中悄悄把“应用数据目录不可写”解释为“部分应用数据目录不可写”。需要对明确的路径冲突作出规格决定，而不是继续补命令文本判断。

## 已执行验证

1. `/usr/bin/sandbox-exec` 存在，真实受限 shell 启动成功。
2. 真实 Python 子进程尝试写 `~/.codefactory/m224-probe-never-created`，得到 PermissionError；随后断言目标文件不存在。
3. 合成临时目录模拟 `app-data/execution-workspaces/session`：
   - 全局 deny-write + workspace allow + app-data deny：shell 写入失败，目标不存在（机器断言通过）。
   - 全局 deny-write + workspace allow，不加 app-data blanket deny：shell 写入成功，内容等于 `probe`（机器断言通过）。
4. 初次测试使用 `/var` 临时目录别名，allow 未匹配；按 `.resolve()` 规范化为 `/private/var` 后重跑相同断言，全部通过。策略生成必须处理系统目录别名。
5. 两次探针产生的合成临时目录均已清理；无后台进程或服务。

## 来源核实

- 系统官方手册 `man sandbox-exec`：支持 `-p` profile-string、`-D` 参数；命令标为 deprecated，存在性与策略加载需要真实探测，不能只查 PATH。
- OpenAI Codex 当前源码：https://raw.githubusercontent.com/openai/codex/main/codex-rs/sandboxing/src/seatbelt.rs
  - 固定使用 `/usr/bin/sandbox-exec`，避免 PATH 注入。
  - 显式处理系统路径别名、可写根目录、可变 symlink 组件。
  - 联网为独立策略，不把文件写入边界宣称为网络隔离。

## AI Collaboration

- Context scope：用户任务与模板、quick/governance profile、权限网关、前台 bash、后台任务启动、工作区注册与 CI。
- Assumptions：所有登记工作区应能正常修改代码；不擅自给受保护应用数据目录加例外。
- Review point：已请求两项独立只读审查（安全边界、测试与 CI）；尚未收到结果，不将创建任务当成审查通过。
- Validation result：路径重叠及真实 Seatbelt 策略冲突已通过独立机器断言复现；产品修复与 R1–R10 验收尚未执行。

## 恢复执行条件

编排方对上述 Spec Amendment 作出产品级决定后，继续同一受管分支，先提交原规格，再写失败测试并实现；保持 macOS CI、前后台实际进程、失败关闭、回退开关与 R10 兼容验收要求不变。真实安装版验收由编排方发版后执行。
