-- M116: a delivery ceiling may be raised inside the same delivery identity.
--
-- The previous rule made `requested_ceiling` immutable, so a run that had been
-- recorded at `pr_only` could never continue at `through_merge` — the caller was
-- told its own delivery had "changed identity". Raising the recorded ceiling is
-- a widening of the same authorized delivery, so it is now allowed; lowering it
-- still is not (M69 / CF-TRUTH-R5: a lowered call delivers less this time and
-- must never rewrite durable authorization downward).
--
-- `sessions.delivery_ceiling` persists the ceiling the user's own dispatch /
-- steering message declared. Before this column, only the boolean
-- `delivery_authorized` survived, so `ceiling: merged` in a kickoff message was
-- discarded and the first delivery fell back to the global setting.

ALTER TABLE sessions ADD COLUMN delivery_ceiling TEXT;

DROP TRIGGER IF EXISTS trg_delivery_runs_requested_ceiling_immutable;

CREATE TRIGGER IF NOT EXISTS trg_delivery_runs_requested_ceiling_no_lower
BEFORE UPDATE OF requested_ceiling ON delivery_runs
WHEN NEW.requested_ceiling <> OLD.requested_ceiling
 AND NOT (
   (CASE NEW.requested_ceiling
      WHEN 'off' THEN 0 WHEN 'pr_only' THEN 1 WHEN 'through_ci_green' THEN 2
      WHEN 'through_merge' THEN 3 WHEN 'through_release' THEN 4 ELSE -1 END)
   >
   (CASE OLD.requested_ceiling
      WHEN 'off' THEN 0 WHEN 'pr_only' THEN 1 WHEN 'through_ci_green' THEN 2
      WHEN 'through_merge' THEN 3 WHEN 'through_release' THEN 4 ELSE -1 END)
 )
BEGIN
    SELECT RAISE(ABORT, 'delivery requested ceiling may only be raised');
END;
