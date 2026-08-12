-- SPDX-License-Identifier: Apache-2.0
-- Durable, exact-scope authorization prompts. A provider tool_call_id is only
-- correlation data: authorization authority is the opaque Objective revision,
-- its immutable binding generation, the hashed action, and prompt generation.

CREATE TABLE IF NOT EXISTS permission_intents (
    intent_id                TEXT PRIMARY KEY,
    objective_id             TEXT NOT NULL
                                 REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision       INTEGER NOT NULL CHECK(objective_revision >= 1),
    binding_id               TEXT NOT NULL
                                 REFERENCES objective_bindings(id) ON DELETE CASCADE,
    resource_generation      INTEGER NOT NULL CHECK(resource_generation >= 1),
    session_id               TEXT NOT NULL CHECK(TRIM(session_id) <> ''),
    provider_tool_call_id    TEXT NOT NULL CHECK(TRIM(provider_tool_call_id) <> ''),
    tool_name                TEXT NOT NULL CHECK(TRIM(tool_name) <> ''),
    prompt_args_json         TEXT NOT NULL CHECK(JSON_VALID(prompt_args_json)),
    action_signature         TEXT NOT NULL CHECK(
                                 LENGTH(action_signature) = 64
                                 AND action_signature NOT GLOB '*[^0-9a-f]*'),
    prompt_generation        INTEGER NOT NULL CHECK(prompt_generation >= 1),
    predecessor_intent_id    TEXT UNIQUE
                                 REFERENCES permission_intents(intent_id),
    status                   TEXT NOT NULL CHECK(status IN (
                                 'pending', 'allowed', 'consumed', 'denied',
                                 'timed_out', 'channel_closed', 'cancelled',
                                 'superseded')),
    failure_code             TEXT,
    expires_at               INTEGER NOT NULL,
    decided_at               INTEGER,
    consumed_at              INTEGER,
    created_process_instance TEXT NOT NULL CHECK(TRIM(created_process_instance) <> ''),
    created_at               INTEGER NOT NULL,
    updated_at               INTEGER NOT NULL,
    CHECK(LENGTH(TRIM(objective_id)) >= 16),
    CHECK(objective_id NOT LIKE 'chat:%'),
    CHECK(objective_id NOT LIKE 'task:%'),
    CHECK(expires_at > created_at),
    CHECK(
      (status IN ('pending', 'allowed', 'consumed') AND failure_code IS NULL)
      OR
      (status = 'denied' AND failure_code = 'permission_denied_by_user')
      OR
      (status = 'timed_out' AND failure_code = 'permission_timed_out')
      OR
      (status = 'channel_closed' AND failure_code = 'permission_channel_closed')
      OR
      (status = 'cancelled' AND failure_code = 'permission_cancelled')
      OR
      (status = 'superseded' AND failure_code = 'permission_scope_stale')
    ),
    CHECK(
      (status = 'pending' AND decided_at IS NULL AND consumed_at IS NULL)
      OR
      (status = 'allowed' AND decided_at IS NOT NULL AND consumed_at IS NULL)
      OR
      (status = 'consumed' AND decided_at IS NOT NULL AND consumed_at IS NOT NULL)
      OR
      (status IN ('denied', 'timed_out', 'channel_closed', 'cancelled', 'superseded')
       AND decided_at IS NOT NULL AND consumed_at IS NULL)
    ),
    UNIQUE(
      objective_id, objective_revision, binding_id, resource_generation,
      action_signature, prompt_generation
    )
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_permission_intents_one_pending_tool_call
    ON permission_intents(
      objective_id, objective_revision, binding_id, resource_generation,
      provider_tool_call_id
    )
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_permission_intents_due
    ON permission_intents(status, expires_at)
    WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS idx_permission_intents_objective
    ON permission_intents(objective_id, objective_revision, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_permission_intents_action_chain
    ON permission_intents(
      objective_id, objective_revision, binding_id, resource_generation,
      action_signature, prompt_generation DESC
    );

-- A projected prompt and the authority to cross a later mutation boundary are
-- deliberately separate records. The prompt remains bound to the Objective
-- revision the user actually saw. An allow response atomically advances the
-- Objective and mints exactly one receipt for that new recovery revision.
CREATE TABLE IF NOT EXISTS permission_action_receipts (
    receipt_id              TEXT PRIMARY KEY,
    intent_id               TEXT NOT NULL UNIQUE
                                REFERENCES permission_intents(intent_id),
    objective_id            TEXT NOT NULL
                                REFERENCES objectives(id) ON DELETE CASCADE,
    objective_revision      INTEGER NOT NULL CHECK(objective_revision >= 1),
    remediation_id          TEXT NOT NULL UNIQUE,
    binding_id              TEXT NOT NULL
                                REFERENCES objective_bindings(id) ON DELETE CASCADE,
    resource_generation     INTEGER NOT NULL CHECK(resource_generation >= 1),
    action_signature        TEXT NOT NULL CHECK(
                                LENGTH(action_signature) = 64
                                AND action_signature NOT GLOB '*[^0-9a-f]*'),
    status                  TEXT NOT NULL CHECK(status IN (
                                'available', 'reserved', 'consumed', 'superseded')),
    consumer_owner          TEXT,
    consumer_claim_epoch    INTEGER,
    consumed_at             INTEGER,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    CHECK(
      (status = 'available' AND consumer_owner IS NULL
       AND consumer_claim_epoch IS NULL AND consumed_at IS NULL)
      OR
      (status = 'reserved' AND consumer_owner IS NOT NULL
       AND consumer_claim_epoch IS NOT NULL AND consumed_at IS NOT NULL)
      OR
      (status = 'consumed' AND consumer_owner IS NOT NULL
       AND consumer_claim_epoch IS NOT NULL AND consumed_at IS NOT NULL)
      OR
      (status = 'superseded' AND consumed_at IS NULL)
    ),
    UNIQUE(
      objective_id, objective_revision, binding_id, resource_generation,
      action_signature
    )
);

CREATE INDEX IF NOT EXISTS idx_permission_action_receipts_recovery
    ON permission_action_receipts(
      objective_id, objective_revision, remediation_id, status
    );

-- Keep the hard identity fence in SQLite as well as application code. This is
-- particularly important after a process crash, when a recovery path may be
-- the first caller to touch the row.
CREATE TRIGGER IF NOT EXISTS trg_permission_intents_scope_insert
BEFORE INSERT ON permission_intents
WHEN NOT EXISTS (
       SELECT 1 FROM objectives
       WHERE id=NEW.objective_id AND revision=NEW.objective_revision
         AND status NOT IN ('completed','cancelled','legacy_orphan')
     )
  OR NOT EXISTS (
       SELECT 1 FROM objective_bindings
       WHERE id=NEW.binding_id AND objective_id=NEW.objective_id
         AND resource_generation=NEW.resource_generation
     )
BEGIN
    SELECT RAISE(ABORT, 'stale permission Objective scope');
END;

CREATE TRIGGER IF NOT EXISTS trg_permission_intents_scope_authority
BEFORE UPDATE OF status ON permission_intents
WHEN NEW.status IN ('allowed', 'consumed')
 AND NOT (
   OLD.status='allowed' AND NEW.status='consumed'
   AND EXISTS (
     SELECT 1 FROM permission_action_receipts receipt
     JOIN objectives objective ON objective.id=receipt.objective_id
     WHERE receipt.intent_id=NEW.intent_id
       AND receipt.objective_id=NEW.objective_id
       AND receipt.binding_id=NEW.binding_id
       AND receipt.resource_generation=NEW.resource_generation
       AND receipt.action_signature=NEW.action_signature
       AND receipt.objective_revision=objective.revision
       AND receipt.status IN ('available','reserved','consumed')
   )
 )
 AND (
   NOT EXISTS (
     SELECT 1 FROM objectives
     WHERE id=NEW.objective_id AND revision=NEW.objective_revision
       AND status NOT IN ('completed','cancelled','legacy_orphan')
   )
   OR NOT EXISTS (
     SELECT 1 FROM objective_bindings
     WHERE id=NEW.binding_id AND objective_id=NEW.objective_id
       AND resource_generation=NEW.resource_generation
   )
 )
BEGIN
    SELECT RAISE(ABORT, 'stale permission Objective scope');
END;

CREATE TRIGGER IF NOT EXISTS trg_permission_intents_predecessor_chain
BEFORE INSERT ON permission_intents
WHEN NEW.predecessor_intent_id IS NOT NULL
 AND NOT EXISTS (
   SELECT 1 FROM permission_intents predecessor
   WHERE predecessor.intent_id=NEW.predecessor_intent_id
     AND predecessor.status IN ('timed_out','channel_closed')
     AND predecessor.objective_id=NEW.objective_id
     AND predecessor.objective_revision<=NEW.objective_revision
     AND predecessor.binding_id=NEW.binding_id
     AND predecessor.resource_generation=NEW.resource_generation
     AND predecessor.provider_tool_call_id=NEW.provider_tool_call_id
     AND predecessor.tool_name=NEW.tool_name
     AND predecessor.prompt_args_json=NEW.prompt_args_json
     AND predecessor.action_signature=NEW.action_signature
     AND predecessor.prompt_generation + 1=NEW.prompt_generation
 )
BEGIN
    SELECT RAISE(ABORT, 'invalid permission prompt predecessor');
END;

CREATE TRIGGER IF NOT EXISTS trg_permission_intents_status_monotonic
BEFORE UPDATE OF status ON permission_intents
WHEN OLD.status <> NEW.status
 AND NOT (
   (OLD.status='pending' AND NEW.status IN (
      'allowed','denied','timed_out','channel_closed','cancelled','superseded'))
   OR (OLD.status='allowed' AND NEW.status IN ('consumed','superseded'))
 )
BEGIN
    SELECT RAISE(ABORT, 'invalid permission intent transition');
END;
