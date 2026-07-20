# CodeFactory heredoc 与可观察状态锁屏验收证据

## 结论

- proof tier: `agent-runtime-no-gui`
- product path: CodeFactory `product` policy、共享 Rust `codefactory-agent-core`、Rust headless sidecar
- screen state: 最终 runtime 开始与结束均为 `CGSSessionScreenIsLocked=Yes`
- model backend: `deepseek / deepseek-v4-pro`
- task type: 非 Terminal-Bench 的 C 源码生成、编译运行、本地 console 后台服务和用户可见状态验证
- contamination: 未使用 benchmark task name、答案、verifier、solution 或 task-specific repair

## Failure-First

单元测试先复现两个共享缺陷：quoted C heredoc 中的 `&value` 被错误分类为后台服务；成功 bounded probe 只输出 transport connection 时，明确要求的用户可见状态仍会提前完成。独立审查随后发现全量剔除 heredoc 会漏掉 `bash <<EOF` 中真实执行的后台命令，自定义或重定义 `cat/tee` 也可能隐藏可执行 heredoc，`no login prompt observed` 等否定输出仍可能误记为成功状态，宽泛的 `output contains` 等入口还会从问题描述中提取错误目标。所有问题均先形成失败测试，再修复。

最终测试覆盖 quoted/strip-tabs 标准数据 heredoc、真实 heredoc 后续 `&`、bash/unquoted/unclosed/piped/process-substituted/custom/redefined heredoc 保守扫描、算术移位、肯定式 `expect to see` / `wait until` 提取，以及 RuntimeProbe / bounded FunctionalProbe、ReadOnly、普通 test 和否定输出的状态观测边界。

第一次真实 product acceptance 的 C 编译和 console probe 都成功，但旧字段 `required_observable_literals=[]`，暴露 `login prompt through a bounded client probe` 被短语提取器丢失。完整原句先形成失败测试，再修复 display-noun 边界；第一次运行不作为 R43 通过证据。

## Harness 配置边界

- 未显式固定 endpoint 的第一次 preflight 在任何模型请求前拒绝 ChatGPT subscription transport，不计产品能力结果。
- 未传 `--allow-network` 的 v3 运行被 macOS sandbox 正确禁止 loopback bind/connect，最终 fail-closed；这是验收能力配置错误，不计产品能力失败。
- 带 `--allow-network` 的 v4-v7 在中间审查或验收口径不够严格的版本上通过，只作为机制样本；以下 v8 才是最终代码和公开验收口径对齐的证据。

## 最终锁屏运行

- evidence dir: `.codefactory/product-acceptance/observable-r42-r43-r8`
- status: `passed`
- duration: `41,882 ms`
- tool calls: `5`
- model requests: `6`
- token usage: prompt `24,768`，completion `1,628`，total `26,396`
- completion blockers: `[]`
- source mutation sequence: `1`
- last failure sequence: `4`（复合启动命令中的系统 `timeout` 缺失，随后以独立 Python probe 恢复）
- service start/log/PID/bounded-probe sequences: `4 / 5 / 5 / 5`
- required observable: `login`
- observed observable: `login`
- source evidence: quoted `cat` heredoc wrote `pointer.c` with `read_through_pointer` and `int *pointer = &value`; `gcc` runtime printed `7`，公开 `python3 verify.py` 输出 `PUBLIC_ACCEPTANCE_OK`，driver 结束后独立重跑仍通过
- functional evidence: final bounded Python socket client captured `Welcome to the local console\nlocalhost login:`；仅 transport 连接不能满足该门禁
- headless binary SHA-256: `c545efde460e002793a8dd37986e7df270ba901385f04268278c26e0bc3496ba`
- execution contract SHA-256: `a0a2c6ffc3c9185199ac16a02671a8b2c564c5d1fb4e914503220f262ecab52b`
- cleanup: runtime 结束后检查精确 PID `6110` 已不存在；没有遗留验收服务

提交前范围审计删除了未由本次 canary 公开需求证明的版本/commit 推断器及其测试，避免把 verifier 误归因扩散到主产品。后续复审指出普通测试输出、否定状态、自定义/重定义数据命令和过宽状态入口；这些边界均已通过 failure-first 测试收敛。合同同步限定为 complete、expansion-disabled、direct standard `cat/tee` data heredoc，其余形式一律 fail-closed。

## 本地回归

- shared core: `84 / 84`
- headless: `15 / 15`
- desktop Rust: `375 passed / 6 ignored`
- governance baseline: pass
- independent review: 多轮只读复审持续找到 heredoc fail-open、跨行/inline command 重定义、否定状态和 anchor 边界问题，均先由失败测试复现后修复。最后两次复审进程未在两分钟内返回终态并被停止；最终候选由 `84/84` failure-first core、headless/desktop 回归、v8 锁屏 Runtime 和 PR 远端 CI/真实 App job 共同门禁，不能声称取得了最后一次独立 clean review

## 证据边界

- 这是锁屏无关的真实模型与共享产品 Agent runtime 证据，不是 GUI 截图或已发布安装包证据。
- R42-R43 仍是本地候选；PR/CI、远端真实 App GUI、合并、deliberate release、published artifact 和 released-build canary 未完成前不得称为已产品化。
- 本轮只使用公开用户指令中的 login-prompt 要求。任何 verifier-only 版本、答案或隐藏断言均未进入产品实现；提交候选也不包含通用版本号推断机制。
