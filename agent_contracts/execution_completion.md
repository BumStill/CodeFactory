# CodeFactory Execution Completion Contract

This contract applies to every autonomous or approved execution run. It is
shared by the desktop Agent and headless evaluation runtime.

## Completion Rules

1. Inspect the real workspace before changing it. Never infer a hidden verifier,
   solution, credential, or host path.
2. After changing code, configuration, generated artifacts, or dependencies,
   run a relevant verification command. A successful verification must be later
   than the last mutation. A prior green result is stale after another change.
   Environment, path, version, or dependency inspection is diagnostic evidence,
   not verification of the requested behavior.
3. For source-build work, prove the build, installation or usable artifact,
   runtime behavior outside the source directory, and project tests when those
   stages apply. Enumerate the actual build inputs from manifests and build
   configuration, including generated or compiled source inputs, and apply a
   compatibility fix across that complete input set. Derive local import aliases
   from the source before the first expensive build or installation instead of
   assuming one canonical module name. Scan every alias across every observed
   source and build-input extension. Expand the compatibility symbol set from
   exact API members reported by real build, runtime, and test failures. Add
   candidate spellings only when they are supported by repository references or
   a language adapter; then batch token-safe, idempotent edits.
   Rerun the same compatibility scan after editing and before rebuilding or
   installing. Write residual matches to a temporary results file, preserve the
   `grep` or `rg` status, reject status greater than `1`, and then finish with
   `test ! -s` or use an equivalent structured exit contract. Do not mask search
   errors or let the normal no-match status turn a clean scan into a failed
   command. Do not rebuild after only
   a partial alias scan. Build or import
   success is insufficient while unresolved matches remain.
   Map every explicitly named
   required component to an executed functional check through its public API.
   Importing or locating a compiled extension, plugin, or native library is not a functional check.
   Do not equate a successful compiler exit with a usable installed result.
   When the request explicitly requires repository or project tests, record a
   successful project-test run after the final source edit, installation, and
   external runtime check; a failed or missing test runner is not completion.
4. For a background service, record its PID and log destination, use bounded
   readiness checks, and run a real client or functional probe. Starting a
   process is not completion.
5. A failed command is diagnostic evidence, not completion. Diagnose, repair,
   and rerun the smallest relevant check. A timeout or tool transport failure
   must remain a failed tool result that the Agent can diagnose; it must not
   crash the execution protocol or be treated as a successful check.
6. Stop only when the requested behavior is verified, a precise external
   blocker requires user action, or the execution budget is exhausted. Report
   the evidence and any remaining limitation without claiming success.
7. Use the current execution environment's network capability when the task
   requires ordinary source or dependency retrieval, while respecting the
   caller's active network policy. Do not invent a stricter network denial or
   bypass a restricted environment.
8. The caller owns the total task deadline unless it explicitly supplies a
   shorter Agent wall timeout. Per-request, per-command, and step bounds remain
   active, but an implicit duplicate wall clock must not terminate a valid
   long-running task early.
9. Reduce model round trips during autonomous work. Batch related workspace
   reads, compatible edits, and focused checks into one bounded tool call when
   their ordering and failure handling remain clear. Do not serialize a list of
   independent one-line inspections into separate model requests.
10. Treat the end of the first third of available execution time as a
   source-delivery checkpoint. After source edits, reserve the remaining two
   thirds for installation, a runtime check from outside the source directory,
   focused project tests, and repairs before further exploration or optional
   scope expansion. After that checkpoint, each successful source edit must be
   followed by installation, each installation by the external runtime check,
   and each successful runtime by focused project tests. A real stage failure
   may open one diagnostic and repair cycle, which then returns to installation.
   Once the current source revision installs successfully, do not install more
   speculative dependencies until the next runtime or test stage reports a
   concrete missing dependency.
11. If a required test runner is unavailable, install that runner and rerun the
   same focused tests before making additional source edits. A masked zero exit
   from a shell pipeline does not turn a missing runner into successful tests.
12. When a legacy project's tests fail at an API supplied by a newly installed
   dependency, inspect the project's declared constraints and prefer a
   reproducible compatible dependency version before adding speculative source
   shims. Rerun the original failing test immediately after either repair.

## Integrity Rules

- Do not branch on benchmark task names, fixed repositories, expected artifact
  names, instruction fingerprints, domain answers, or success markers.
- Do not read benchmark verifier, solution, or hidden-test paths.
- Product and evaluation runtimes must record this contract's SHA-256 and the
  completion evidence used for the final decision.
