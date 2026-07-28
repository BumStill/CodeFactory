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
   not verification of the requested behavior. When the request explicitly says
   the user expects to see or wait for a visible state, the functional probe must
   capture that state after the last relevant mutation or service start; process
   liveness, an open port, transport connection, and command acknowledgement are
   insufficient. When the request specifies an expected output or return value,
   the post-change verification must contain a machine-checked assertion, real
   test runner, or dedicated verifier that exits nonzero on mismatch. Printing
   expected and actual values is diagnostic evidence, not successful
   verification. A file-existence, executable, PID, or other precondition
   assertion does not verify requested output unless the target command's
   actual output is captured or piped into the assertion. Examples copied from the request are smoke checks, not
   sufficient completion evidence for behavior over variable inputs. Also run
   a project test, dedicated verifier, generated or property check, or at least
   one machine-checked non-example case whose asserted data comes from the same
   target under test. Run a verifier or inline assertion as a standalone command
   or in a fail-closed `&&` chain. A target-output pipeline is verification only
   when its final stage is the machine assertion; piping a verifier or assertion
   into a non-checking consumer masks its exit status. Suffixes such as
   `; echo $?`, general `||` recovery, `|| true`, and `|| :` mask failures and
   are not verification. An inline interpreter
   assertion is verification only when it does not write the workspace and its
   nonzero exit status reaches the tool result. Discovery-only test modes such as no-run, collect, list,
   help, or version are not execution evidence. Treat executable interpreter or shell heredocs as opaque
   state-changing actions; identifiers such as `test = ...` inside their payload
   must never masquerade as a shell assertion, and a separate machine-checked
   verification must follow them.
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
   When the request specifies a named tool, library, model, version, or revision, the
   implementation and verification must exercise that exact named dependency;
   importing it while performing the work through an adjacent lower-level
   dependency is not compliance.
   Do not equate a successful compiler exit with a usable installed result.
   When the request explicitly requires repository or project tests, record a
   successful project-test run after the final source edit, installation, and
   external runtime check; a failed or missing test runner is not completion.
   Record source installation, the external runtime check, and project tests
   as separate tool calls in that order. A compound command cannot provide
   stage ordering evidence and will be rejected at the delivery checkpoint.
4. For a background service, record its PID and log destination, use bounded
   readiness checks, and run a real client or functional probe. Starting a
   process is not completion. Classify background execution from shell control
   syntax only. Ignore control syntax inside a heredoc only when the complete,
   expansion-disabled payload is direct source data for the standard `cat` or
   `tee` command. Executable, unquoted, piped, process-substituted, redefined,
   custom-command, or unclosed heredocs remain fail-closed. For any state-changing
   control path, capture before-and-after observable state and assert that the
   requested change occurred; a command acknowledgement alone is not functional
   evidence.
5. A failed command is diagnostic evidence, not completion. Diagnose, repair,
   and rerun the smallest relevant check. A timeout or tool transport failure
   must remain a failed tool result that the Agent can diagnose; it must not
   crash the execution protocol or be treated as a successful check. When an
   inline assertion traceback identifies the assertion that actually failed, a
   later successful check may close that failure by replaying the same assertion
   inside a broader fail-closed verification command. Unrelated successful
   assertions must not close it.
6. A policy-, permission-, user-, or hook-denied tool is not an executed action.
   Preserve an auditable, credential-redacted `command`, `rule`, and `reason` for
   every denial. After one fully non-executable batch, require one permitted
   replacement tool call. If the required response contains no executable tool,
   or the next batch is still fully non-executable, stop incomplete with the
   denial decision and remaining evidence instead of consuming the remaining
   model or wall budget.
7. Stop only when the requested behavior is verified, a precise external
   blocker requires user action, or the execution budget is exhausted. Report
   the evidence and any remaining limitation without claiming success.
8. Use the current execution environment's network capability when the task
   requires ordinary source or dependency retrieval, while respecting the
   caller's active network policy. Do not invent a stricter network denial or
   bypass a restricted environment.
9. The caller owns the total task deadline unless it explicitly supplies a
   shorter Agent wall timeout. Per-request, per-command, and step bounds remain
   active, but an implicit duplicate wall clock must not terminate a valid
   long-running task early. When the caller supplies a hard host deadline, the
   Agent must stop starting model requests and tool calls before the caller's
   cleanup reserve begins, return incomplete evidence with the latest usage,
   and terminate any child or in-flight tool process that does not exit within
   that deadline. No child may continue working against a workspace after the
   caller starts workspace cleanup.
10. Reduce model round trips during autonomous work. Batch related workspace
   reads, compatible edits, and focused checks into one bounded tool call when
   their ordering and failure handling remain clear. Do not serialize a list of
   independent one-line inspections into separate model requests. The inspection
   budget applies again after every mutation: once the bounded post-change read
   window is exhausted, make the smallest corrective edit or run a bounded
   functional verification before continuing pure inspection. A mutation or
   functional probe opens a new bounded inspection window; an earlier mutation
   does not permit unlimited later reads. A failed read or runtime probe still
   consumes this diagnostic window and must not reset it.
11. Treat the end of the first third of available execution time as a
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
   For a long task that names a required output artifact, create the first
   candidate artifact before the final third of the budget, then spend the
   remaining time validating and repairing it. Research, dependency setup, or
   repeated inspection must not consume the final third while that artifact is
   still missing.
12. If a required test runner is unavailable, install that runner and rerun the
   same focused tests before making additional source edits. A masked zero exit
   from a shell pipeline does not turn a missing runner into successful tests.
13. When a legacy project's tests fail at an API supplied by a newly installed
   dependency, inspect the project's declared constraints and prefer a
   reproducible compatible dependency version before adding speculative source
   shims. Rerun the original failing test immediately after either repair.
14. In the final third of a host-supplied wall-clock execution budget, or the
   final eight model rounds when the host exposes only a round budget, an
   unresolved failed mutation or check permits one bounded
   read-only diagnostic. After that diagnostic, make the smallest corrective
   mutation or rerun a focused machine check; do not continue read-only
   exploration. A new real failure may open one new diagnostic, while only a
   successful mutation, closure of the failed check at the same or broader
   scope, or another material reduction in completion blockers resets this
   diagnostic-stagnation state. It does not reset the per-turn cumulative
   budget for rejected final-response recovery. Failed tools, diagnostic
   reads, and unrelated green checks
   do not reset it. If the
   completion decision rejects a text-only final response, the next response
   must execute a bounded tool call that directly resolves a blocker unless a
   precise external blocker requires user action. If the provider accepts tools
   but rejects forced tool selection, retry once with automatic tool selection;
   the local state machine must still stop incomplete when no tool is returned.
   A tool call rejected by policy is not an executed action and must not clear
   the required-tool state. Record its command, rule, and reason, require one
   bounded permitted replacement, and stop incomplete if the next tool batch is
   also non-executable. Recovery must state how to preserve verifier exit status
   before allowing another read or edit. Repeated text-only analysis or policy-denied batches
   must not consume the remaining execution budget.

## Integrity Rules

- Do not branch on benchmark task names, fixed repositories, expected artifact
  names, instruction fingerprints, domain answers, or success markers.
- Do not read benchmark verifier, solution, or hidden-test paths.
- Product and evaluation runtimes must record this contract's SHA-256 and the
  completion evidence used for the final decision.
