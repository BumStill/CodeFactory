### Background and user decision
The model the user chooses (including the default model set in settings) is a decision made with cost and quality in mind; the model the system actually uses must match what the user selected and what the UI shows. The sidebar title must be enough to tell sessions apart.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-MSH-R1 | A new session's model = the model shown in the draft's picker at the moment it's sent; when the user didn't choose explicitly, it equals the current default model in settings; it doesn't use any other cached old value | Tests: default / explicitly selected / default changed then create / after an app restart, four cases |
| CF-MSH-R2 | When the default model in settings changes, the default shown in the next new draft and the model actually used change together | Test + real-browser check (screenshot of the picker + database model field) |
| CF-MSH-R3 | Once fallback happens, the session tells the user which model is actually answering (already partly present); and after the preferred model recovers, a new turn uses the selected model again rather than the fallback | Test |
| CF-TTL-R5 | The temporary title is derived from the first message's content (summarise or truncate key words) so that different sessions can be told apart; a generic category name that doesn't reflect the content must not be used | Table-driven tests: different first messages → different, readable temporary titles |

### Applicable Harnesses
Spec Harness; Viewport Harness (model picker / sidebar title, real browser); Compatibility Harness (existing session data); AI Collaboration Harness.
