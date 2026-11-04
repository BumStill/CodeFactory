// SPDX-License-Identifier: Apache-2.0
/**
 * M236 test backstop: an in-memory stand-in for the durable queued-dispatch
 * commands, so store-level tests keep exercising the real queue state machine
 * (claim → send → confirm) instead of a bare `vi.fn()` that returns `undefined`.
 *
 * Production never uses this file; it is imported only by tests that mock the
 * Tauri invoke channel.
 */

export interface QueuedRow {
  id: string;
  sessionId: string;
  content: string;
  rootTurnId: string;
  status: "queued" | "delivering" | "delivered" | "failed";
  attempts: number;
  createdAt: number;
  updatedAt: number;
  deliveredAt: number | null;
  failureReason: string | null;
  position: number | null;
}

const rows: QueuedRow[] = [];
let sequence = 0;

function pendingOf(sessionId: string): QueuedRow[] {
  return rows
    .filter(
      (row) =>
        row.sessionId === sessionId && (row.status === "queued" || row.status === "delivering"),
    )
    .sort((a, b) => a.createdAt - b.createdAt);
}

function withPosition(row: QueuedRow): QueuedRow {
  const pending = pendingOf(row.sessionId);
  const index = pending.findIndex((item) => item.id === row.id);
  return { ...row, position: index >= 0 ? index + 1 : null };
}

/** Pure dispatch table: `handled: false` means "not a queued-dispatch command". */
export function queuedDispatchCommand(
  command: string,
  args: Record<string, unknown> = {},
): { handled: boolean; value?: unknown } {
  const sessionId = (args.sessionId as string) ?? "";
  const id = (args.id as string) ?? (args.messageId as string) ?? "";
  switch (command) {
    case "enqueue_queued_dispatch_message": {
      const pending = pendingOf(sessionId);
      if (pending.length >= 5) throw new Error("queue is full");
      sequence += 1;
      const now = Date.now() + sequence;
      const row: QueuedRow = {
        id: `queued-${sequence}`,
        sessionId: sessionId,
        content: String(args.content ?? "").trim(),
        rootTurnId: (args.rootTurnId as string) ?? `root-queued-${sequence}`,
        status: "queued",
        attempts: 0,
        createdAt: now,
        updatedAt: now,
        deliveredAt: null,
        failureReason: null,
        position: pending.length + 1,
      };
      rows.push(row);
      return { handled: true, value: { ...row } };
    }
    case "list_queued_dispatch_messages":
      return { handled: true, value: pendingOf(sessionId).map(withPosition) };
    case "claim_next_queued_dispatch_message": {
      const next = pendingOf(sessionId).find((row) => row.status === "queued");
      if (!next) return { handled: true, value: null };
      next.status = "delivering";
      next.attempts += 1;
      next.updatedAt = Date.now();
      return { handled: true, value: { ...next } };
    }
    case "mark_queued_dispatch_message_delivered": {
      const row = rows.find((item) => item.id === id && item.sessionId === sessionId);
      if (row) {
        row.status = "delivered";
        row.deliveredAt = Date.now();
        row.position = null;
      }
      return { handled: true, value: row ? { ...row } : null };
    }
    case "mark_queued_dispatch_message_failed": {
      const row = rows.find((item) => item.id === id && item.sessionId === sessionId);
      if (row) {
        row.status = "failed";
        row.failureReason = String(args.reason ?? "");
        row.position = null;
      }
      return { handled: true, value: row ? { ...row } : null };
    }
    case "fail_pending_queued_dispatch_messages": {
      const pending = pendingOf(sessionId);
      for (const row of pending) {
        row.status = "failed";
        row.failureReason = String(args.reason ?? "");
        row.position = null;
      }
      return { handled: true, value: pending.map((row) => ({ ...row })) };
    }
    case "dispatch_message_status": {
      const row = rows.find((item) => item.id === id && item.sessionId === sessionId);
      return { handled: true, value: row ? withPosition(row) : null };
    }
    case "list_sessions_with_queued_dispatch_messages":
      return {
        handled: true,
        value: [...new Set(rows.filter((row) => row.status === "queued").map((row) => row.sessionId))],
      };
    default:
      return { handled: false };
  }
}

/** Point an invoke mock at the backend above; anything else resolves `undefined`. */
export function installQueuedDispatch(
  mock: { mockImplementation: (fn: (...args: never[]) => unknown) => unknown },
): void {
  mock.mockImplementation((async (command: string, args?: Record<string, unknown>) => {
    const result = queuedDispatchCommand(command, args ?? {});
    return result.handled ? result.value : undefined;
  }) as never);
}

/** Test isolation: forget every row between cases. */
export function resetQueuedDispatch(): void {
  rows.length = 0;
  sequence = 0;
}

export function queuedRows(): QueuedRow[] {
  return rows.map((row) => ({ ...row }));
}
