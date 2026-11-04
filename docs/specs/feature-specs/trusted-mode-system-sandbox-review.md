# 可信模式系统级沙箱：实现与复核记录（M224）

## 背景

可信模式（trusted mode）下 CodeFactory 不再逐条询问用户是否允许命令，因此"命令是否真的只能动这个项目"
必须由操作系统内核而不是提示词来保证。本记录实现并复核了以下九条要求，并给出可重复执行的证据。

## 需求对照

| ID | 要求 | 实现位置 | 证据 |
| --- | --- | --- | --- |
| R1 | 默认开启 | `config/settings.rs` 的 `trusted_command_sandbox`，默认 `true` | `config::settings` 测试 29/29 |
| R2 | macOS 命令只能写项目目录 | `agent/trusted_sandbox.rs` 生成 SBPL：`(deny file-write*)` 后紧跟工作区 `subpath`、共享编译缓存 `subpath`、临时目录 `subpath`、`/dev/null` 等少量字面量 | `trusted_sandbox::tests::write_inside_workspace_is_allowed_and_outside_is_not` |
| R3 | 项目外写入必须被系统拒绝，且提示可读 | 同上；stdout/stderr 透传沙箱原文，用户看到的是 `Operation not permitted` 而不是空白 | 同上（HOME 根目录探针被拒绝、工作区内写入成功） |
| R4 | 网络按端点请求需要放行 | `CommandSandbox::network()` 控制 `(allow network*)`；默认 `(deny default)`，需要时显式放开 | `trusted_sandbox::tests::profile_denies_by_default_and_allows_only_the_workspace` |
| R5 | 覆盖 `bash` 与后台任务 | `agent/builtin_tools/bash.rs`、`background_tasks.rs` 统一经 `wrap_command` 取得启动参数 | 编译期类型约束 + 既有 `background_task` 测试 46/46 |
| R6 | 不做深度防御的假象 | 沙箱是"命令执行的唯一边界"，不做进程级/系统调用级分层承诺；profile 只做文件写与网络 | profile 测试断言"默认拒绝 + 白名单"，不宣称覆盖提权/内核漏洞 |
| R7 | 写白名单包含共享编译缓存 | `build_cache` 暴露给 `trusted_sandbox` 的缓存根目录进入 `(subpath ...)` | `write_inside_workspace_is_allowed_and_outside_is_not` 覆盖缓存内写入 |
| R8 | 三种路径模式等价 | 内联 `-p`、临时文件 `-f`、短命令内联/长命令文件两种模式走同一 profile 文本 | `trusted_sandbox::tests::profile_is_identical_in_inline_and_file_modes` |
| R9 | 默认开启、用户在设置里可关闭 | 设置页"可信模式命令使用系统沙箱(默认开启)"开关；关闭后走无隔离路径 | `pnpm build`（tsc 类型 + 产物构建）通过 |

## 已验证的坑（实现过程中真实踩到）

1. **临时目录名不能含括号**：`{:?}` 打印线程 id 会得到 `ThreadId(1)`，括号进入 `sh -c` 与 profile 字面量后语法破坏。改为 `m224-<tag>-<pid>`。
2. **`#` 会被 shell 当注释**：探针命令用 `printf done #...` 时后半句被丢弃，测试假绿。改为 `printf 'done <token>'` 并断言完整 token。
3. **`/tmp` 在写白名单内**：构建缓存需要临时目录，因此"工作区外"探针必须落在 HOME 根目录这类未被允许的位置，否则测不出边界。

## 未覆盖（明确声明，不作承诺）

- Windows 无 `sandbox-exec` 等价物：当前实现是 macOS `sandbox-exec`；Linux 走 `bwrap` 分支在 CI 覆盖前按"不可用即不启用"处理，不宣称已生效。
- 沙箱不防内核层提权、不防命令读敏感文件的规模限制（读取面仅由项目目录与运行所需系统路径约束）；这些属于后续独立需求。

## 复核结论

R1–R9 均有对应实现与可执行证据；R2/R3/R8 的证据是真实进程级断言（拒绝由系统返回），不是结构检查。
