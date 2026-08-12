-- SPDX-License-Identifier: Apache-2.0
-- Durable recovery contracts for native mutations that are not covered by
-- the exact-content file observer (0012), Browser (0014), or DeliveryRun.
--
-- The contract stores only opaque Objective identity, bounded resource
-- locators, and digests.  It never stores raw shell commands, document
-- content, skill instructions, credentials, or provider payloads.

CREATE TABLE IF NOT EXISTS tool_recovery_contracts (
    receipt_id                  TEXT PRIMARY KEY
                                      REFERENCES side_effect_receipts(id)
                                      ON DELETE CASCADE,
    objective_id               TEXT NOT NULL
                                      REFERENCES objectives(id)
                                      ON DELETE CASCADE,
    objective_revision         INTEGER NOT NULL CHECK(objective_revision >= 1),
    binding_id                 TEXT NOT NULL
                                      REFERENCES objective_bindings(id)
                                      ON DELETE CASCADE,
    resource_generation        INTEGER NOT NULL CHECK(resource_generation >= 1),
    action_fingerprint         TEXT NOT NULL CHECK(
                                      LENGTH(action_fingerprint) = 71
                                      AND SUBSTR(action_fingerprint, 1, 7) = 'sha256:'
                                      AND SUBSTR(action_fingerprint, 8)
                                          NOT GLOB '*[^0-9a-f]*'),
    tool_call_id               TEXT NOT NULL CHECK(tool_call_id <> ''),
    resource_kind              TEXT NOT NULL CHECK(resource_kind IN (
                                      'workspace_file', 'workspace_git',
                                      'session_plan', 'session_tasks',
                                      'user_skills')),
    replay_policy              TEXT NOT NULL CHECK(replay_policy IN (
                                      'exact_if_unchanged', 'never_after_dispatch')),
    safe_locator_json          TEXT NOT NULL CHECK(
                                      JSON_VALID(safe_locator_json)
                                      AND JSON_TYPE(safe_locator_json) = 'object'
                                      AND LENGTH(safe_locator_json) <= 2048),
    precondition_digest        TEXT NOT NULL CHECK(
                                      LENGTH(precondition_digest) = 71
                                      AND SUBSTR(precondition_digest, 1, 7) = 'sha256:'
                                      AND SUBSTR(precondition_digest, 8)
                                          NOT GLOB '*[^0-9a-f]*'),
    postcondition_digest       TEXT CHECK(postcondition_digest IS NULL OR (
                                      LENGTH(postcondition_digest) = 71
                                      AND SUBSTR(postcondition_digest, 1, 7) = 'sha256:'
                                      AND SUBSTR(postcondition_digest, 8)
                                          NOT GLOB '*[^0-9a-f]*')),
    state                      TEXT NOT NULL CHECK(state IN (
                                      'prepared', 'dispatching', 'committed', 'unknown',
                                      'observed_unchanged', 'observed_changed',
                                      'still_unknown', 'settled_committed',
                                      'settled_reconciled', 'cancelled')),
    dispatch_owner             TEXT,
    dispatch_claim_epoch       INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_claim_epoch >= 0),
    dispatch_generation        INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_generation >= 0),
    observation_count          INTEGER NOT NULL DEFAULT 0 CHECK(observation_count >= 0),
    dispatch_started_at        INTEGER,
    observed_at                INTEGER,
    settled_at                 INTEGER,
    created_at                 INTEGER NOT NULL,
    updated_at                 INTEGER NOT NULL,
    CHECK(
      (state = 'prepared' AND dispatch_generation = 0
       AND dispatch_owner IS NULL AND dispatch_claim_epoch = 0
       AND dispatch_started_at IS NULL)
      OR (state = 'cancelled' AND dispatch_generation = 0
          AND dispatch_owner IS NULL AND dispatch_claim_epoch = 0
          AND dispatch_started_at IS NULL)
      OR (state NOT IN ('prepared', 'cancelled')
          AND dispatch_generation >= 1 AND dispatch_started_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_tool_recovery_scope
    ON tool_recovery_contracts(
      objective_id, objective_revision, binding_id, resource_generation, state
    );

-- 0012 predates exact provider-call attribution.  Keep one common link so a
-- restart can terminalize the crash-left normalized row and write its replay
-- message before resuming the same Objective.  The first call remains the
-- owner; later exact retries use the same receipt but get their own ordinary
-- foreground result.
CREATE TABLE IF NOT EXISTS tool_recovery_call_links (
    receipt_id   TEXT NOT NULL
                      REFERENCES side_effect_receipts(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY(receipt_id, tool_call_id)
);

-- The reconciliation decision itself is a crash boundary.  Persist it before
-- restarting Chat/Task so a process death between "observe" and "resume"
-- returns the same decision instead of losing retry/replan authority.
CREATE TABLE IF NOT EXISTS tool_recovery_reconciliations (
    receipt_id      TEXT PRIMARY KEY
                         REFERENCES side_effect_receipts(id) ON DELETE CASCADE,
    remediation_id  TEXT NOT NULL UNIQUE
                         REFERENCES objective_remediations(id) ON DELETE CASCADE,
    claim_epoch     INTEGER NOT NULL CHECK(claim_epoch >= 1),
    disposition     TEXT NOT NULL CHECK(disposition IN ('retry_exact', 'replan_current_state')),
    created_at      INTEGER NOT NULL
);

CREATE TRIGGER IF NOT EXISTS trg_tool_recovery_outer_scope_insert
BEFORE INSERT ON tool_recovery_contracts
WHEN NOT EXISTS (
  SELECT 1 FROM side_effect_receipts receipt
  WHERE receipt.id = NEW.receipt_id
    AND receipt.objective_id = NEW.objective_id
    AND receipt.revision = NEW.objective_revision
    AND receipt.binding_id = NEW.binding_id
    AND receipt.action_fingerprint = NEW.action_fingerprint
    AND receipt.status IN ('started', 'unknown')
)
BEGIN
  SELECT RAISE(ABORT, 'tool recovery contract does not match its outer receipt');
END;

-- Locators are deliberately vocabulary-limited.  A future tool must add an
-- explicit observer instead of smuggling arbitrary arguments into recovery.
CREATE TRIGGER IF NOT EXISTS trg_tool_recovery_safe_locator_insert
BEFORE INSERT ON tool_recovery_contracts
WHEN EXISTS (
  SELECT 1 FROM JSON_EACH(NEW.safe_locator_json)
  WHERE key NOT IN ('workspace_relative_path', 'session_id', 'root_turn_id')
     OR type <> 'text'
     OR LENGTH(value) = 0
     OR LENGTH(value) > 1024
)
BEGIN
  SELECT RAISE(ABORT, 'tool recovery locator contains unsupported fields');
END;

CREATE TRIGGER IF NOT EXISTS trg_tool_recovery_receipt_terminal
BEFORE UPDATE OF status ON side_effect_receipts
WHEN NEW.status IN ('committed', 'reconciled')
 AND EXISTS (
   SELECT 1 FROM tool_recovery_contracts contract
   WHERE contract.receipt_id = NEW.id
     AND (
       (NEW.status = 'committed' AND contract.state <> 'settled_committed')
       OR (NEW.status = 'reconciled' AND contract.state <> 'settled_reconciled')
     )
 )
BEGIN
  SELECT RAISE(ABORT, 'tool receipt cannot settle before its recovery contract');
END;
