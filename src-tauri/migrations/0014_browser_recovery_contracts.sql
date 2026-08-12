-- SPDX-License-Identifier: Apache-2.0
-- Durable, privacy-preserving recovery contracts for managed browser actions.
--
-- `side_effect_receipts` remains the outer mutation fence. Every browser row
-- is its one-to-one domain contract and carries only opaque internal identity
-- plus digests. Raw URLs, tab ids, fill text and browser pairing credentials
-- have no column in this schema and are rejected from the safe locator.

CREATE TABLE IF NOT EXISTS browser_recovery_contracts (
    receipt_id                    TEXT PRIMARY KEY
                                          REFERENCES side_effect_receipts(id)
                                          ON DELETE CASCADE,
    objective_id                 TEXT NOT NULL
                                          REFERENCES objectives(id)
                                          ON DELETE CASCADE,
    objective_revision           INTEGER NOT NULL CHECK(objective_revision >= 1),
    binding_id                   TEXT NOT NULL
                                          REFERENCES objective_bindings(id)
                                          ON DELETE CASCADE,
    resource_generation          INTEGER NOT NULL CHECK(resource_generation >= 1),
    action_fingerprint           TEXT NOT NULL CHECK(
                                          LENGTH(action_fingerprint) = 71
                                          AND SUBSTR(action_fingerprint, 1, 7) = 'sha256:'
                                          AND SUBSTR(action_fingerprint, 8)
                                              NOT GLOB '*[^0-9a-f]*'),
    tool_call_id                 TEXT NOT NULL CHECK(tool_call_id <> ''),
    action                       TEXT NOT NULL CHECK(action IN (
                                          'click', 'fill', 'press', 'open',
                                          'attach', 'select_tab', 'close', 'screenshot')),
    replay_policy                TEXT NOT NULL CHECK(replay_policy IN (
                                          'never_after_dispatch',
                                          'exact_generation', 'digest_cas')),
    session_id                   TEXT NOT NULL CHECK(
                                          session_id <> '' AND INSTR(session_id, '://') = 0),
    session_generation           INTEGER NOT NULL CHECK(session_generation >= 1),
    observer_kind                TEXT NOT NULL CHECK(observer_kind IN (
                                          'session_presence_v1', 'page_digest_v1',
                                          'element_digest_v1', 'tab_digest_v1',
                                          'workspace_file_sha256_v1')),
    safe_locator_json            TEXT NOT NULL CHECK(
                                          LENGTH(safe_locator_json) <= 2048),
    precondition_digest          TEXT CHECK(
                                          precondition_digest IS NULL OR (
                                            LENGTH(precondition_digest) = 64
                                            AND precondition_digest NOT GLOB '*[^0-9a-f]*')),
    expected_postcondition_digest TEXT CHECK(
                                          expected_postcondition_digest IS NULL OR (
                                            LENGTH(expected_postcondition_digest) = 64
                                            AND expected_postcondition_digest NOT GLOB '*[^0-9a-f]*')),
    state                        TEXT NOT NULL CHECK(state IN (
                                          'prepared', 'dispatching', 'acknowledged',
                                          'unknown', 'observed_applied',
                                          'observed_not_applied', 'conflict',
                                          'settled_committed', 'settled_reconciled',
                                          'cancelled')),
    dispatch_owner               TEXT,
    dispatch_claim_epoch         INTEGER NOT NULL DEFAULT 0
                                          CHECK(dispatch_claim_epoch >= 0),
    dispatch_generation          INTEGER NOT NULL DEFAULT 0
                                          CHECK(dispatch_generation >= 0),
    last_observation             TEXT CHECK(last_observation IS NULL OR
                                          last_observation IN (
                                            'applied', 'definitely_not_applied',
                                            'still_unknown', 'conflict')),
    observed_digest              TEXT CHECK(observed_digest IS NULL OR (
                                          LENGTH(observed_digest) = 64
                                          AND observed_digest NOT GLOB '*[^0-9a-f]*')),
    observation_count            INTEGER NOT NULL DEFAULT 0
                                          CHECK(observation_count >= 0),
    dispatch_started_at          INTEGER,
    acknowledged_at              INTEGER,
    observed_at                  INTEGER,
    settled_at                   INTEGER,
    created_at                   INTEGER NOT NULL,
    updated_at                   INTEGER NOT NULL,
    CHECK(
      (action IN ('click','fill','press')
       AND replay_policy = 'never_after_dispatch')
      OR (action IN ('open','attach','select_tab','close')
          AND replay_policy = 'exact_generation')
      OR (action = 'screenshot' AND replay_policy = 'digest_cas')
    ),
    CHECK(NOT (
      action IN ('click','fill','press')
      AND state = 'observed_not_applied'
    )),
    CHECK(
      (state = 'prepared' AND dispatch_generation = 0
       AND dispatch_owner IS NULL AND dispatch_claim_epoch = 0
       AND dispatch_started_at IS NULL)
      OR (state = 'cancelled' AND dispatch_generation = 0
          AND dispatch_owner IS NULL AND dispatch_claim_epoch = 0
          AND dispatch_started_at IS NULL)
      OR (state NOT IN ('prepared', 'cancelled') AND dispatch_generation >= 1
          AND dispatch_started_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_browser_recovery_scope
    ON browser_recovery_contracts(
      objective_id, objective_revision, binding_id, resource_generation, state
    );

CREATE INDEX IF NOT EXISTS idx_browser_recovery_session
    ON browser_recovery_contracts(session_id, session_generation, state);

-- The domain row can only be created for the exact outer receipt identity and
-- while that receipt is still unresolved. As the receipt id is the primary
-- key this is also the one-to-one cardinality fence.
CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_outer_scope_insert
BEFORE INSERT ON browser_recovery_contracts
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
  SELECT RAISE(ABORT, 'browser contract does not match its outer receipt');
END;

-- Only a small digest vocabulary is accepted. This is deliberately a trigger
-- rather than application validation so a future caller cannot persist a raw
-- URL, tab id, fill value, pairing token or arbitrary provider payload.
CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_safe_locator_insert
BEFORE INSERT ON browser_recovery_contracts
WHEN NOT JSON_VALID(NEW.safe_locator_json)
  OR JSON_TYPE(NEW.safe_locator_json) <> 'object'
  OR EXISTS (
    SELECT 1 FROM JSON_EACH(NEW.safe_locator_json)
    WHERE key NOT IN (
      'session_digest', 'document_digest', 'target_digest',
      'tab_digest', 'path_digest', 'focus_digest'
    )
      OR type <> 'text'
      OR LENGTH(value) <> 64
      OR value GLOB '*[^0-9a-f]*'
  )
BEGIN
  SELECT RAISE(ABORT, 'browser safe locator must contain digests only');
END;

CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_identity_immutable
BEFORE UPDATE OF receipt_id, objective_id, objective_revision, binding_id,
                 resource_generation, action_fingerprint, tool_call_id,
                 action, replay_policy, session_id, session_generation,
                 observer_kind, safe_locator_json
ON browser_recovery_contracts
BEGIN
  SELECT RAISE(ABORT, 'browser recovery identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_precondition_once
BEFORE UPDATE OF precondition_digest ON browser_recovery_contracts
WHEN NOT (
  OLD.action IN ('click', 'press')
  AND OLD.state = 'prepared'
  AND OLD.dispatch_generation = 0
  AND OLD.precondition_digest IS NULL
  AND NEW.precondition_digest IS NOT NULL
  AND LENGTH(NEW.precondition_digest) = 64
  AND NEW.precondition_digest NOT GLOB '*[^0-9a-f]*'
  AND OLD.expected_postcondition_digest IS NOT NULL
  AND NEW.precondition_digest <> OLD.expected_postcondition_digest
)
BEGIN
  SELECT RAISE(ABORT, 'browser dispatch precondition is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_expected_postcondition_once
BEFORE UPDATE OF expected_postcondition_digest ON browser_recovery_contracts
WHEN NOT (
  OLD.action = 'screenshot'
  AND OLD.replay_policy = 'digest_cas'
  AND OLD.state = 'dispatching'
  AND OLD.expected_postcondition_digest IS NULL
  AND NEW.expected_postcondition_digest IS NOT NULL
  AND LENGTH(NEW.expected_postcondition_digest) = 64
  AND NEW.expected_postcondition_digest NOT GLOB '*[^0-9a-f]*'
)
BEGIN
  SELECT RAISE(ABORT, 'browser expected postcondition is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_state_forward
BEFORE UPDATE OF state ON browser_recovery_contracts
WHEN NOT (
  OLD.state = NEW.state
  OR (OLD.state = 'prepared' AND NEW.state IN ('dispatching', 'cancelled'))
  OR (OLD.state = 'dispatching'
      AND (NEW.state IN ('acknowledged', 'unknown', 'observed_applied', 'conflict')
           OR (NEW.state = 'observed_not_applied'
               AND OLD.replay_policy IN ('exact_generation', 'digest_cas'))))
  OR (OLD.state = 'unknown'
      AND NEW.state IN ('observed_applied', 'observed_not_applied', 'conflict'))
  OR (OLD.state = 'observed_not_applied' AND NEW.state = 'dispatching'
      AND OLD.replay_policy IN ('exact_generation', 'digest_cas'))
  OR (OLD.state = 'acknowledged' AND NEW.state = 'settled_committed')
  OR (OLD.state = 'observed_applied' AND NEW.state = 'settled_reconciled')
)
BEGIN
  SELECT RAISE(ABORT, 'browser recovery state cannot move backward');
END;

-- Updating an outer receipt cannot silently detach it from its browser scope.
CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_outer_identity_immutable
BEFORE UPDATE OF objective_id, binding_id, revision, action_fingerprint
ON side_effect_receipts
WHEN EXISTS (
  SELECT 1 FROM browser_recovery_contracts contract
  WHERE contract.receipt_id = OLD.id
    AND (
      contract.objective_id <> NEW.objective_id
      OR contract.binding_id <> NEW.binding_id
      OR contract.objective_revision <> NEW.revision
      OR contract.action_fingerprint <> NEW.action_fingerprint
    )
)
BEGIN
  SELECT RAISE(ABORT, 'browser outer receipt identity is immutable');
END;

-- Generic callers cannot declare a browser mutation committed merely because
-- a driver future returned. The domain contract must first persist an ack or
-- an applied observation.
CREATE TRIGGER IF NOT EXISTS trg_browser_recovery_outer_settlement_gate
BEFORE UPDATE OF status ON side_effect_receipts
WHEN EXISTS (
  SELECT 1 FROM browser_recovery_contracts contract
  WHERE contract.receipt_id = NEW.id
    AND (
      (NEW.status = 'committed'
       AND contract.state NOT IN ('acknowledged', 'settled_committed'))
      OR (NEW.status = 'reconciled'
          AND contract.state NOT IN ('observed_applied', 'settled_reconciled'))
    )
)
BEGIN
  SELECT RAISE(ABORT, 'browser outer receipt lacks durable settlement evidence');
END;
