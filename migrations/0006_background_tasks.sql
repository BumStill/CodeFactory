-- CF-BG 阶段 1：会话级后台任务（R1 后台命令 + R3 管理与上限）。
-- 规格：docs/specs/feature-specs/session-background-tasks.md
CREATE TABLE IF NOT EXISTS background_tasks (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    root_turn_id TEXT,
    command TEXT NOT NULL,
    cwd TEXT NOT NULL,
    source TEXT NOT NULL,
    status TEXT NOT NULL,
    exit_code INTEGER,
    pid INTEGER,
    process_started_at TEXT,
    output_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    timeout_sec INTEGER NOT NULL,
    wake_state TEXT NOT NULL DEFAULT 'none'
);

CREATE INDEX IF NOT EXISTS idx_background_tasks_session ON background_tasks(session_id, status);
CREATE INDEX IF NOT EXISTS idx_background_tasks_status ON background_tasks(status);
CREATE INDEX IF NOT EXISTS idx_background_tasks_wake ON background_tasks(wake_state);
