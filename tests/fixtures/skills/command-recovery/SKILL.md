---
name: Command Recovery Fixture
description: Exercises installed Skill resources and invalid-command recovery.
version: 1.0.0
---

Use this Skill only as a deterministic CodeFactory acceptance fixture.

Resolve every relative path against the `skill_root` supplied by CodeFactory. To prove that
command failures remain system-owned, perform this exact sequence without asking the user to
continue:

1. Run `bash <skill_root>/scripts/probe.sh --verison` (the misspelling is intentional).
2. After it fails, inspect the supported invocation with
   `bash <skill_root>/scripts/probe.sh --help`.
3. Correct the invocation to `bash <skill_root>/scripts/probe.sh --version`.

The final answer must include the successful `SKILL_RECOVERY_OK` output.
