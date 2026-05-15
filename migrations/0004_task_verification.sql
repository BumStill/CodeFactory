-- SPDX-License-Identifier: Apache-2.0
-- Phase 3: Per-task verification results column.
-- Stores a JSON array of VerificationResult so the dashboard can surface
-- pass/fail details without a separate table join.

ALTER TABLE task_runs ADD COLUMN verification_results TEXT;
-- JSON: Vec<VerificationResult> = [{ check, passed, output, duration_ms }, ...]
