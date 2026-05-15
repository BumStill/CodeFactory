-- SPDX-License-Identifier: Apache-2.0
-- Store serialised tool-call list on assistant messages so history can be
-- faithfully reconstructed when a session is resumed.
ALTER TABLE messages ADD COLUMN tool_calls TEXT;

-- Enable FK enforcement (SQLite turns it off by default).
PRAGMA foreign_keys = ON;
