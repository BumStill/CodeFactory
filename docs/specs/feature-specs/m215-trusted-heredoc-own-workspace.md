# M215：trusted 模式下，只改本会话工作区文件的解释器程序不应询问

Release-Urgency: default

## 背景

2026-10-31 会话 `9df929d4`（M211）在 trusted（全程允许）模式下被两次弹出人工审批：

- `cd "$(pwd)" && python3 - <<'PY' … io.open('src-tauri/src/agent/thin_derive.rs') …` → `trusted.heredoc.unproved`
- `cd "$(pwd)" && git merge origin/main --no-edit 2>&1 | tail -5 && git log --oneline -3` → `trusted.git.unproved`

两条命令都只触碰本会话自己的执行工作区，属于系统技术状态，按
`OBJECTIVE_MANUAL_TECHNICAL_HANDOFF` 应当由系统自行判定，而不是转成人工产品级决策。
根因是三处能力缺口，而不是用户配置问题：

1. `python_heredoc_proof` 要求头部严格等于 `python3 - <<'PY'`（此前只额外放过
   `cd "$(pwd)"; ` 这一种写法），真实写法 `cd "$(pwd)" && python3 - <<'PY'` 直接判为不可证明；
2. AST 证明器不支持 `io.open`、`pathlib.Path(...).read_text/write_text`、`with`、
   `import io/pathlib`，也不支持工作区内绝对路径——这些恰恰是模型最常写的文件编辑形状；
3. Git 证明器在 `segments()` 里遇到 `$(pwd)` 全局返回 `None`（命令替换一律拒绝），
   于是 `cd "$(pwd)" && git merge …` 永远走不到已证明的 `merge origin/main --no-edit` 分支。

## 需求

- **R1 只读/只写本工作区的解释器程序在 trusted 模式下直接放行**：证明器识别
  `open` / `io.open` / `pathlib.Path(...).read_text|write_text`，只接受相对路径或
  工作区内绝对路径；`subprocess`、`os.system`、`shutil.rmtree`、网络（`urllib`/`socket`/
  `requests`）、`eval`/`exec`、`..` 越界、指向工作区外的绝对路径一律不可证明。
- **R2 不可证明时失败必须带原因行**：证明器把无法证明的脚本行号与原因写进拒绝理由
  （`第N行无法证明：…`），审批卡片据此说明卡在哪一行；不带原因的整体拒绝视为未实现。
- **R3 系统提示词引导优先使用 `edit_file`/`write_file`**，heredoc 只在脚本确实更合适时使用，
  且必须保持在工作区内。提示词只是引导，不能替代 R1 的证明能力。
- **R4 `cd "$(pwd)"` 前缀可证明**：`cd "$(pwd)"` 只是重新进入当前目录（物理路径同一棵树），
  因此 Git 证明器在分词前剥离该前缀；其它命令替换（`$(...)`、反引号）继续 fail closed。
  heredoc 头部同样接受 `cd <本工作区根> &&` 与 `cd "$(pwd)" &&`，目标必须是登记根。

- **R5 heredoc 前面的只读命令链不再导致拒绝**：`cd <本工作区> && grep -rl X . | head; echo ===;
  python3 - <<'PY'` 这种「先跑几段只读命令、最后才是 heredoc」的写法，头部按顶层分隔符
  （`;`、`|`、`&&`、换行，引号内不切分）拆段，除最后一段外每段都必须是只读命令
  （`grep`/`head`/`echo`/`wc`/`cat` 等）、只读 Git，或落回本工作区的 `cd`；任何写操作
  （重定向、`rm`、`git reset`、`sed -i`）或 `cd` 到工作区外仍然整体不可证明。
  这条来自 11-03 的现场样本：`cd <本工作区> && grep -rl X . | head; echo ===; python3 - <<'PY'`
  被报 `trusted.heredoc.unproved`，理由「不接受的解释器入口」。

## 验收

- `src-tauri/tests/m198_trusted_heredoc.rs`（生产解析器直连）：
  `m215_own_workspace_heredoc_is_proved`（3 种头部 × 4 种脚本形状全部 `Ok(())`）、
  `m215_unproved_heredoc_names_the_line_and_stays_closed`（7 种越界/外部能力 + 3 种越界 cd
  全部 `Err`，且错误文本含 `行无法证明`）、
  `m215_registered_workspace_merge_started_with_pwd_cd_is_proved`。
- `src-tauri/src/agent/trusted_git.rs`：`m215_trusted_heredoc_and_read_only_git` 单元模块
  覆盖 R1/R2/R3/R5——`field_shapes_are_proved`（现场 5 种形状）、`boundaries_still_ask`
  （越界/越权 6 种仍拒绝）、`read_only_git_needs_no_approval_but_writes_still_do`
  （只读 Git 放行、写操作仍敏感）、`read_only_prefix_before_heredoc_is_proved`
  （只读命令链 + heredoc 放行；`rm`/`git reset`/越界 `cd` 仍拒绝）。
- `src-tauri/src/agent/permission_gateway.rs`：M198 真实执行测试追加 M215 形状，断言
  `PermissionOutcome::Allow`。
- 既有 M198/R2 拦截用例（`import os`、`../outside`、未闭合 heredoc、heredoc 之后仍有 shell）
  必须全部保持不可证明。
- 跨平台 stdio：证明器把 stdin/stdout/stderr 固定为 UTF-8（旧代码页会把中文诊断输出成
  `\u7b2c2\u884c...`），`trusted_git.rs` 再用 `decode_prover_escapes` 兜底还原 `\uXXXX`，
  保证 Windows 上的审批理由同样是可读中文行号。回归用例
  `m215_prover_reason_survives_a_legacy_console_code_page` 以 `PYTHONIOENCODING=cp1252`
  运行证明器并断言 stderr 是未转义的 UTF-8。

## 证据

- `cargo test --test m198_trusted_heredoc`：36 passed（含 25 条既有 M198/trusted 用例）。
- `cargo test --lib m215`：M215 真实解释器执行用例（含新增 M215 断言）4 passed。
- `cargo test --test m159_trusted_git --test m177_trusted_local_git --test m180_readonly_still_prompts
  --test m182_trusted_git_denylist --test m207_trusted_git_composition`：全部 ok（33/32/30/33/30）。
- 直接运行 `trusted_python_ast.py`：6 条放行样例 exit 0，7 条拦截样例 exit 1 且带行号。
- `sensitive()` 不带工作区根（单元层）时同样成立：`cd "$(pwd)"` 前缀按"就是当前目录"放行，
  因此 `m198_quoted_python_is_not_shell_syntax` 无需根也能证明，不再退化成 `trusted.heredoc.unproved`。
