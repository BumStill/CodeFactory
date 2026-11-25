"""M283：执行产品 split 退休 SQL 的 SQLite 行为回归，不替代 Rust 入口测试。"""
import pathlib
import re
import sqlite3
import unittest


class SplitRetirement(unittest.TestCase):
    def test_running_turn_is_not_rebound(self):
        source = pathlib.Path("src-tauri/src/agent/execution_workspace.rs").read_text()
        body = source.split("async fn repair_session_split_locked", 1)[1].split("async fn repair_session_split_under_repo_lock", 1)[0]
        sql = re.search(r'"(UPDATE chat_turn_state SET objective_id=.*?)"', body).group(1)
        db = sqlite3.connect(":memory:")
        db.execute("CREATE TABLE chat_turn_state(objective_id TEXT, session_id TEXT, status TEXT, terminal_reason TEXT)")
        db.executemany("INSERT INTO chat_turn_state VALUES (?,?,?,?)", [
            ("donor", "session", "working", None),
            ("donor", "session", "settled", "completed"),
            ("donor", "other", "working", None),
        ])
        db.execute(sql, ("keeper", "donor", "session"))
        self.assertEqual(db.execute("SELECT objective_id FROM chat_turn_state WHERE session_id='session' AND status='working'").fetchone()[0], "donor")
        self.assertEqual(db.execute("SELECT objective_id FROM chat_turn_state WHERE status='settled'").fetchone()[0], "keeper")
        self.assertEqual(db.execute("SELECT objective_id FROM chat_turn_state WHERE session_id='other'").fetchone()[0], "donor")


if __name__ == "__main__":
    unittest.main()
