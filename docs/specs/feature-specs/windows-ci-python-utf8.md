# Windows CI 上 Python 治理脚本的 UTF-8 编码契约（M49）

## 背景

同类故障已第三次出现在 Windows runner 上，且每次都掩盖了真实错误：

- 2026-10-10（#593）：`validate_readme_contract.py` 打印中文错误时 `UnicodeEncodeError`（`cp1252`），真正的校验错误被编码异常顶掉。
- 2026-10-17（#680）：`run_flaky_retry.py` 读取子进程输出时 `UnicodeDecodeError: 'charmap' codec can't decode byte 0x8f`，`check-frontend` 因此失败。

根因一致：Windows runner 上 Python 默认 locale 编码是 `cp1252`，而仓库里的治理脚本会读取/输出非 ASCII 文本（中文错误串、git 输出、子进程日志）。逐个脚本打补丁会继续漏，因此在环境层一次性兜底，并加防回归检查。

## 决策

1. **环境层兜底**：`.github/workflows/` 下所有工作流在 workflow 级声明 `PYTHONUTF8: "1"` 与 `PYTHONIOENCODING: utf-8`，不依赖具体 runner 的 locale。
2. **进程层兜底**：`tools/`、`scripts/` 中被 CI 调用的 Python 脚本，凡是 text 模式调用子进程（`text=True` / `universal_newlines=True`）的，一律显式给出 `encoding="utf-8", errors="replace"`，不再继承平台默认编码。
3. **防回归**：新增 `tools/governance/validate_python_utf8_contract.py`，既检查工作流的 env，也检查 text 模式子进程调用的 encoding；它与既有 governance 基线一起在 `governance-baseline` 必需检查里运行。
4. **不靠环境变量掩盖脚本缺陷**：`validate_readme_contract.py` 自身在 `PYTHONIOENCODING=cp1252`、`PYTHONUTF8=0` 下也必须能输出中文错误文本而不是崩溃。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 所有在 Windows runner 上运行 Python 的工作流 job 统一启用 UTF-8 模式，覆盖 `.github/workflows/` 下全部工作流（workflow 级 `env: PYTHONUTF8: "1"` + `PYTHONIOENCODING: utf-8` 或等价方式）。 | `validate_python_utf8_contract.py` 对真实工作流目录的检查通过；单元测试覆盖“缺 env → 失败”；逐文件确认 13 个工作流均已声明。 |
| R2 | 仓库内被 CI 调用的 Python 脚本中，`subprocess.run/Popen/check_output` 使用 `text=True` / `universal_newlines=True` 却未指定 `encoding=` 的，补 `encoding="utf-8", errors="replace"`。 | 检查器对 `tools/`、`scripts/` 扫描为 0 违规；单元测试覆盖“text 模式无 encoding → 失败”与“已声明 encoding → 通过”。 |
| R3 | 增加防回归检查（并入现有 governance 校验/CI lint）：(a) 新增或修改的工作流缺 R1 env 时失败；(b) `tools/` 与 `scripts/` 下 Python 出现 text 模式 subprocess 却没写 encoding 时失败；检查本身要有单元测试，含先失败的用例。 | `tests/test_python_utf8_contract.py` 全部通过（含先失败用例）；`python3 tools/governance/validate_python_utf8_contract.py` 退出码 0；在真实仓库上先看到失败再看到通过。 |
| R4 | 回归测试：在 `PYTHONIOENCODING=cp1252 PYTHONUTF8=0` 环境下，`validate_readme_contract.py` 输出中文错误不崩溃并保留错误文本。 | `tests/test_python_utf8_contract.py::ReadmeContractCp1252Tests` 以子进程真实调用 CLI，断言退出码正常、`UnicodeEncodeError` 不出现、中文错误文本出现在输出中。 |

## Applicable Harnesses

- **Spec Harness**：本文件即规格，R1–R4 与测试矩阵一一对应。
- **Compatibility Harness**：编码契约只影响 CI 脚本运行时编码，不改变任何 CLI 参数、退出码语义或必需检查集合。
- **AI Collaboration Harness**：context scope（M49 任务文件 + 仓库治理文档 + 现有工作流/脚本）、assumptions（Windows runner 默认 `cp1252`；workflow 级 env 覆盖所有 job）、review point（是否为所有工作流加 env、是否放宽任何检查）、validation result（见 PR 的自验收表）。
- **不适用**：Payload / Viewport / Release Harness（无产品代码、无 UI、无发布产物变化）。

## 测试矩阵

| 用例 | 类型 | 断言 |
| --- | --- | --- |
| T-R1-1 | 单元 | 工作流缺少 `PYTHONUTF8` / `PYTHONIOENCODING` 时，检查器返回违规（先失败用例）。 |
| T-R1-2 | 单元 | 工作流声明了顶层 `env` 时，检查器通过。 |
| T-R1-3 | 单元 | job 级 env 声明也算等价（R1 的“或等价方式”）。 |
| T-R1-4 | 集成 | 真实 `.github/workflows/` 全部通过。 |
| T-R2-1 | 单元 | `text=True` 无 `encoding=` → 违规（先失败用例）。 |
| T-R2-2 | 单元 | `text=True, encoding="utf-8", errors="replace"` → 通过。 |
| T-R2-3 | 单元 | `universal_newlines=True` 同样受约束。 |
| T-R2-4 | 集成 | 真实 `tools/`、`scripts/` 扫描 0 违规。 |
| T-R3-1 | 集成 | CLI 在真实仓库退出码 0，输出含 `pass`。 |
| T-R4-1 | 回归 | `PYTHONIOENCODING=cp1252 PYTHONUTF8=0` 下 CLI 输出中文错误文本、无 `UnicodeEncodeError`。 |

## Implementation Notes

- 检查器按 AST 解析 Python（而不是正则扫文本），避免注释、字符串里的示例被误判。
- 工作流检查按 YAML 结构判断：顶层或任一 job 上的 `env` 同时包含 `PYTHONUTF8=1` 与 `PYTHONIOENCODING=utf-8` 即视为满足；不解析具体 step。
- `validate_readme_contract.py` 在启动时把 `stdout`/`stderr` 重配为 UTF-8，使脚本在恶劣 locale 下仍能输出完整错误文本。
