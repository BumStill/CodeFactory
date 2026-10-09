# Tool rejection settlement (U29)

## Background and user decision
The user wants to make the subscription GPT the default and fall back to DeepSeek when it's unavailable. Different models use different tools; any tool the system offers the model must either be usable, or fail without affecting what follows.

## Decision
A tool call that is rejected or fails before any side effect happens is recorded as "no effect" and never leaves later steps in an "external state uncertain" wait. Tools offered to the model must be usable in the current session; tools that aren't usable are not offered.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-TRS-R1 | Any tool call that is refused before execution (unavailable, failed parameter validation, refused by policy, unknown tool, etc.) settles its side-effect record as "no effect", never leaving "unknown" | Table-driven test: each kind of pre-execution rejection → record is "no effect" |
| CF-TRS-R2 | After such a rejection, the following file edits / commands in the same task run normally, with no "external state uncertain" wait | Synthetic end-to-end: rejected tool → edit_file succeeds right after |
| CF-TRS-R3 | The tool list offered to the model contains only tools usable in the current session; `delegate_tasks` is either really usable in a normal project session, or not offered there at all (whichever you choose, explain the reason in the PR) | Tests: normal project session, sub-session, unbound session |
| CF-TRS-R4 | The same tool, with arguments identical in meaning, offered under different models (ChatGPT / DeepSeek / OpenRouter) behaves the same | Test covering all three endpoint shapes |
| CF-TRS-R5 | Existing receipts left at "unknown" by such rejections (data already present) no longer block continuing; history isn't rewritten | Compatibility test |

## Applicable Harnesses
Spec Harness; Compatibility Harness (existing receipt data); AI Collaboration Harness.

## Test matrix
- Rejection types: tool unavailable / invalid parameters / refused by policy / unknown tool name.
- Continuation: edit, run a command, deliver right after the rejection.
- Model shapes: ChatGPT (responses), DeepSeek (OpenAI-compatible), OpenRouter.
- Real side effect happened and then failed (e.g. command half-run): must still be treated as "uncertain" (counter-example; must not be weakened).

## Constraints
- Must not weaken the "really uncertain" case: a tool that may already have affected the outside world must still be reconciled before continuing.
- **This task itself must not call `delegate_tasks`** (it's broken right now; calling it will just trap you in this bug).
- Other constraints are as in the workflow template.
