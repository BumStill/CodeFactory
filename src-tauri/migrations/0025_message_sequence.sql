-- MEM-A (阶段 A): 为会话消息建立稳定的排序身份与内容版本。
--
-- 背景：`messages.created_at` 是秒级的，而主键是 `id TEXT`（不是
-- `INTEGER PRIMARY KEY`），所以同一秒内的先后只能靠隐式 `rowid` 决定，而
-- `rowid` 在 VACUUM 之后不保证保持。也就是说「会话里的第 7 条消息」目前不是
-- 一个稳定事实，任何依赖它的分级恢复/摘要都无法自证。
--
-- 本迁移只做加法：新增两列、回填、加索引与两个触发器。不删除、不重写任何既有
-- 数据列，旧版本代码读到的 `SELECT m.*` 行为不变（新列可空/有默认值）。

ALTER TABLE messages ADD COLUMN seq INTEGER;
ALTER TABLE messages ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

-- 回填：以迁移前实际生效的先后规则 (created_at, rowid) 为准，逐会话编号。
-- 必须在创建 revision 触发器之前完成，否则回填本身会被当成内容变更。
UPDATE messages
SET seq = (
    SELECT numbered.rn
    FROM (
        SELECT
            m2.id AS message_id,
            ROW_NUMBER() OVER (
                PARTITION BY m2.session_id
                ORDER BY m2.created_at ASC, m2.rowid ASC
            ) AS rn
        FROM messages m2
    ) AS numbered
    WHERE numbered.message_id = messages.id
);

CREATE INDEX IF NOT EXISTS messages_session_seq
    ON messages (session_id, seq);

-- 新写入：写事务内取会话内最大值 +1。SQLite 只有一个写者，`MAX(seq)` 在同一
-- 事务里读到的是已提交状态，因此并发写不会撞号，也不会乱序。
CREATE TRIGGER IF NOT EXISTS messages_assign_seq
AFTER INSERT ON messages
WHEN NEW.seq IS NULL
BEGIN
    UPDATE messages
    SET seq = COALESCE(
        (SELECT MAX(m.seq) FROM messages m WHERE m.session_id = NEW.session_id),
        0
    ) + 1
    WHERE messages.id = NEW.id;
END;

-- 内容版本：只有真正改变回放内容或 gate 语义的列才升版本，token 记账等列不算。
CREATE TRIGGER IF NOT EXISTS messages_bump_revision
AFTER UPDATE OF content, role, tool_calls, reasoning_content, completion_state ON messages
WHEN NEW.revision = OLD.revision
 AND (
      NEW.content IS NOT OLD.content
   OR NEW.role IS NOT OLD.role
   OR NEW.tool_calls IS NOT OLD.tool_calls
   OR NEW.reasoning_content IS NOT OLD.reasoning_content
   OR NEW.completion_state IS NOT OLD.completion_state
 )
BEGIN
    UPDATE messages
    SET revision = OLD.revision + 1
    WHERE messages.id = NEW.id;
END;

-- 灰度开关（阶段 A 唯一的开关来源）。缺省无行 = 关闭。
CREATE TABLE IF NOT EXISTS feature_flags (
    name TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0
);

-- 请求装配回执（影子模式）：只记录"这次请求到底发了什么、基于哪些版本"。
CREATE TABLE IF NOT EXISTS assembly_manifests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    flag_enabled INTEGER NOT NULL DEFAULT 1,
    source_count INTEGER NOT NULL,
    excluded_count INTEGER NOT NULL,
    wire_fingerprint TEXT NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS assembly_manifests_session
    ON assembly_manifests (session_id, created_at);
