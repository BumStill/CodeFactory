# Backlog

Things noted during work that aren't in the current PR. Sorted newest-first.
When picking one up, move it to its own PR scope and remove from here.

## Chat input — queue + attachments

The chat / interjection input should support:

- **Message queueing** — typing while the AI is busy queues the next
  message; it fires automatically when the model becomes idle, instead
  of being lost or requiring the user to wait and re-send.
- **File attachments via drag-and-drop** — drag any file from Finder /
  Explorer onto the input area and it's attached to the next message.
- **Screenshot paste** — `Cmd/Ctrl+V` of a clipboard image attaches it
  directly (no save-to-file roundtrip). Same flow for any image on the
  clipboard.

Open questions to resolve when scoping:
- Where do attachments live during a session — temp dir, app data dir,
  or inlined in the message row as base64?
- How are attachments passed to the model — multipart upload, base64
  inline, or URL? Depends on whether OpenRouter endpoint supports vision
  for the active model.
- Queue UI: a small badge "2 queued" next to the input? Click to peek /
  reorder / cancel?
