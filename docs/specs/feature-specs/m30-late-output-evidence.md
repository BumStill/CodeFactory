# M30 验收与交付记录

规格：docs/specs/feature-specs/m30-late-output.md。任务未提供 Req ID 或指定规格路径，本文仅给任务要求补追踪编号，无待决 Spec Amendment。

## Req 与自验收
- CF-M30-R1 pass：chatEvents.late-output.test.ts 完成/取消/失败后 30 秒工具结果及文本被忽略；先见 5 项失败，后见 6 项通过。hydration 任一同 root 分段的水位线可拦截，既有 chat.hydration.test.ts 13 项通过。
- CF-M30-R2 pass：旧 root 迟到输出不污染下一轮，下一轮文本正常，waiting_system 可继续；既有 chatEvents.test.ts 终态/steer 兼容。Rust stream_payload_retains_root_and_original_event_fields 验证生产序列化字段及旧协议。
- CF-M30-R3 pass：pnpm test:turn-handback:headless，生产 reducer + MessageList，真实浏览器断言迟到内容不可见、新回合文本正常。
- pnpm test：151 files / 1006 tests pass，含 lightModeAudit。pnpm build pass。
- CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m30" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：pass。
- 全量前端测试曾发现历史 id 扫描性能回归，改为尾部查找后原测试与全套通过。

## Harnesses / AI Collaboration
Spec：上述映射。Compatibility：复用持久终态字段与无 source 旧事件兼容。Observation：迟到事件仅记类型调试日志，不含内容。Payload：生产输出字段断言。Viewport：真实 Chrome headless，1366x768 / 430x780，浅深色。Release：仅合并不发版。
Context scope：chat reducer、desktop sink、既有 SQLite hydration；assumptions：复用已有持久水位线；review point：根回合隔离、等待不误杀、长会话性能；validation result：完整前端/Rust、构建、真浏览器通过。按模板单执行流，未调用 delegate_tasks。

截图目录：/var/folders/k9/w_s6rw316n7gvz454845pgfc0000gn/T/codefactory-turn-handback-headless，文件 m30-{dark,light}-{wide,narrow}.png。

真机验收由编排方发版后完成：完成/失败/取消回合后核对无迟到文本/工具；开启下一轮核对正常输出；重开会话核对终态保留。当前 not live，本任务不发版。无新 GUI/dispatch 操作入口需求。

Scenario-Test: SC-001
README-Update: reviewed
README-Update-Reason: 内部事件终态隔离修复，不改变公开能力、安装和隐私承诺。
