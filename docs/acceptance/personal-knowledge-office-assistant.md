# M192 检索层逐条验收账本

对应规格：docs/specs/feature-specs/personal-knowledge-office-assistant.md 的 CF-KB-R2 检索层修订（M192）。以下 R1–R5 仅指该章节，不替代历史 M191 的独立验收。

| Req | 状态 | 证据 |
| --- | --- | --- |
| R1 | done | src-tauri/src/knowledge.rs::tests::m192_cjk_mixed_highlight_and_lifecycle；中文短语、词序、部分词、英文大小写、混合、单字与来源字段通过；tools::knowledge::tests::kb_search_uses_attached_database_and_returns_source_json 通过。 |
| R2 | done | src-tauri/src/knowledge.rs::tests::m192_bm25_title_filename_and_length；标题、文件名加权与长度归一化通过，R1 断言纯文本高亮。 |
| R3 | done | src-tauri/src/knowledge.rs::tests::m192_migration_and_20k_p95；迁移和重复初始化通过；m192_cjk_mixed_highlight_and_lifecycle 覆盖更新/删除/回滚/禁用库；cargo test --lib knowledge:: 共 20 项通过。 |
| R4 | done | src-tauri/src/knowledge.rs::tests::m192_migration_and_20k_p95；20,000 合成 chunk，40 次 search，p95 < 200ms 机器断言通过，EXPLAIN FTS 虚拟索引断言通过。 |
| R5 | done | docs/specs/feature-specs/personal-knowledge-office-assistant.md 的 Implementation Notes，记录 FTS5 trigram、jieba 与重叠 bigram 取舍。 |

## 综合验证
- 实现前新增三项测试均失败；实现后全部通过。
- Rust workspace --no-fail-fast 全部通过；源码外执行编译好的 m192_ 测试三项通过；相关知识库回归 20 项通过。
- pnpm test：163 文件、1089 项通过（含 lightModeAudit）；pnpm build 通过。
- scenario harness local、场景声明 --ci、README contract、治理 baseline 通过。
- 不发版，not live。真实安装版由编排方发版后验收：合成 Office/PDF 注册、中文/英文/混合 kb_search、来源字段和高亮、更新删除及禁用过滤、retrieval_events。
- 无待决 Spec Amendment。按任务模板单执行流，不调用 delegate_tasks。
