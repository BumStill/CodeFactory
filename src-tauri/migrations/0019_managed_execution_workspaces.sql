-- SPDX-License-Identifier: Apache-2.0
-- Objective-owned primary Git worktrees. Session cwd remains the user-selected
-- project entry; mutation-capable runs bind to this durable identity instead.

CREATE TABLE IF NOT EXISTS execution_workspaces (
    id                 TEXT PRIMARY KEY,
    objective_id       TEXT NOT NULL UNIQUE REFERENCES objectives(id) ON DELETE CASCADE,
    session_id         TEXT,
    repo_identity      TEXT NOT NULL,
    repo_root          TEXT NOT NULL,
    git_common_dir     TEXT NOT NULL,
    worktree_path      TEXT NOT NULL UNIQUE,
    worktree_identity  TEXT UNIQUE,
    branch_name        TEXT NOT NULL,
    base_ref           TEXT NOT NULL,
    base_sha           TEXT NOT NULL,
    head_sha           TEXT,
    state              TEXT NOT NULL CHECK(state IN (
                           'allocating', 'active', 'delivering',
                           'cleanup_pending', 'closed', 'incident')),
    canonical_pr_number INTEGER,
    canonical_pr_url   TEXT,
    lease_owner        TEXT,
    lease_expires_at   INTEGER,
    failure_code       TEXT,
    failure_detail     TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL,
    closed_at          INTEGER,
    UNIQUE(repo_identity, branch_name)
);

CREATE INDEX IF NOT EXISTS idx_execution_workspaces_state
    ON execution_workspaces(state, lease_expires_at, updated_at);
CREATE INDEX IF NOT EXISTS idx_execution_workspaces_repo
    ON execution_workspaces(repo_identity, updated_at);

CREATE TABLE IF NOT EXISTS execution_workspace_repo_locks (
    repo_identity    TEXT PRIMARY KEY,
    lease_owner     TEXT NOT NULL,
    lease_expires_at INTEGER NOT NULL,
    acquired_at     INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);
