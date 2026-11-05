// SPDX-License-Identifier: Apache-2.0
/**
 * M236 — queued sends are durable, and delivery no longer depends on the
 * renderer that happened to accept them.
 *
 * The old queue lived in `runtime[sessionId].queue` (in-memory) and was drained
 * only when *this* webview reduced a `turn_settled` event for that session.
 * A turn that ended through a scheduled wake-up path never produced that
 * transition here, so the message stayed in an array nobody read and vanished
 * on the next hydration. `dispatch send mode=queue` had already answered
 * `status: "queued"`, so the caller believed it had been handed over.
 *
 * Now: the row is committed in the app database before `queued` is returned,
 * and delivery is a claim → send → confirm handshake against that row. Any
 * drainer (settlement, wake-up, app start) can pick it up; the atomic claim
 * makes a double delivery impossible, and a failed delivery is a visible
 * terminal state with a reason instead of silence.
 */
import { invoke } from "./tauri";

export type QueuedSendStatus = "queued" | "delivering" | "delivered" | "failed";

export interface QueuedSendRecord {
  id: string;
  sessionId: string;
  content: string;
  rootTurnId: string;
  status: QueuedSendStatus;
  attempts: number;
  createdAt: number;
  updatedAt: number;
  deliveredAt: number | null;
  failureReason: string | null;
  position: number | null;
}

/** Storage + delivery bookkeeping. Injectable so the drain logic is testable
 *  without a Tauri runtime. */
export interface QueuedSendPort {
  enqueue(sessionId: string, content: string, rootTurnId?: string): Promise<QueuedSendRecord>;
  list(sessionId: string): Promise<QueuedSendRecord[]>;
  claimNext(sessionId: string): Promise<QueuedSendRecord | null>;
  markDelivered(sessionId: string, id: string): Promise<void>;
  markFailed(sessionId: string, id: string, reason: string): Promise<void>;
  status(sessionId: string, id: string): Promise<QueuedSendRecord | null>;
  sessionsWithPending(): Promise<string[]>;
  failPending(sessionId: string, reason: string): Promise<QueuedSendRecord[]>;
}

export const QUEUE_FULL_MESSAGE = "queue is full";
export const SESSION_ENDED_MESSAGE = "session has ended; queued message was not delivered";

export const tauriQueuedSendPort: QueuedSendPort = {
  enqueue: (sessionId, content, rootTurnId) =>
    invoke<QueuedSendRecord>("enqueue_queued_dispatch_message", {
      sessionId,
      content,
      rootTurnId: rootTurnId ?? null,
    }),
  list: (sessionId) =>
    invoke<QueuedSendRecord[]>("list_queued_dispatch_messages", { sessionId }),
  claimNext: (sessionId) =>
    invoke<QueuedSendRecord | null>("claim_next_queued_dispatch_message", { sessionId }),
  markDelivered: (sessionId, id) =>
    invoke("mark_queued_dispatch_message_delivered", { sessionId, id }).then(() => undefined),
  markFailed: (sessionId, id, reason) =>
    invoke("mark_queued_dispatch_message_failed", { sessionId, id, reason }).then(() => undefined),
  status: (sessionId, id) =>
    invoke<QueuedSendRecord | null>("dispatch_message_status", { sessionId, messageId: id }),
  sessionsWithPending: () =>
    invoke<string[]>("list_sessions_with_queued_dispatch_messages"),
  failPending: (sessionId, reason) =>
    invoke<QueuedSendRecord[]>("fail_pending_queued_dispatch_messages", { sessionId, reason }),
};

export type EnqueueOutcome =
  | { ok: true; record: QueuedSendRecord }
  | { ok: false; reason: "full" | "ended" | "unknown"; message: string };

/**
 * Persist a queued send. `ok: true` is only returned after the row is committed,
 * so callers may report `queued` to a dispatch caller honestly.
 */
export async function enqueueDurableQueuedSend(
  port: QueuedSendPort,
  sessionId: string,
  content: string,
  rootTurnId?: string,
  options: { sessionEnded?: boolean } = {},
): Promise<EnqueueOutcome> {
  const text = content.trim();
  if (!text) return { ok: false, reason: "unknown", message: "message is empty" };
  if (options.sessionEnded) {
    return { ok: false, reason: "ended", message: SESSION_ENDED_MESSAGE };
  }
  try {
    const record = await port.enqueue(sessionId, text, rootTurnId);
    return { ok: true, record };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes(QUEUE_FULL_MESSAGE)) {
      return { ok: false, reason: "full", message };
    }
    return { ok: false, reason: "unknown", message };
  }
}

export interface DrainResult {
  delivered: QueuedSendRecord[];
  failed: { record: QueuedSendRecord; reason: string } | null;
  /** Nothing was claimable — the caller may run its normal post-turn work. */
  empty: boolean;
}

/**
 * Deliver queued messages for one session, oldest first.
 *
 * `limit` defaults to 1: the first message becomes the next turn, and the rest
 * keep waiting for *that* turn to settle — the same ordering the UI shows.
 */
export async function drainQueuedSends(
  port: QueuedSendPort,
  sessionId: string,
  send: (content: string, rootTurnId: string) => Promise<void>,
  options: { limit?: number; failReasonPrefix?: string } = {},
): Promise<DrainResult> {
  const limit = options.limit ?? 1;
  const delivered: QueuedSendRecord[] = [];
  for (let attempt = 0; attempt < limit; attempt += 1) {
    const record = await port.claimNext(sessionId);
    if (!record) {
      return { delivered, failed: null, empty: delivered.length === 0 };
    }
    try {
      await send(record.content, record.rootTurnId);
    } catch (error) {
      const raw = error instanceof Error ? error.message : String(error);
      const reason = `${options.failReasonPrefix ?? "queued message was not delivered"}: ${raw}`;
      await port.markFailed(sessionId, record.id, reason);
      return { delivered, failed: { record, reason }, empty: false };
    }
    await port.markDelivered(sessionId, record.id);
    delivered.push(record);
  }
  return { delivered, failed: null, empty: delivered.length === 0 };
}

/**
 * R3: user/orchestrator messages go before any scheduled wake-up. A wake-up
 * handler calls this first; when it delivered something the wake-up must not
 * also start a turn.
 */
export async function drainBeforeWake(
  port: QueuedSendPort,
  sessionId: string,
  send: (content: string, rootTurnId: string) => Promise<void>,
): Promise<boolean> {
  const result = await drainQueuedSends(port, sessionId, send);
  return result.delivered.length > 0;
}

/** App start: sessions that still owe a delivery resume instead of forgetting. */
export async function sessionsOwingDelivery(port: QueuedSendPort): Promise<string[]> {
  return port.sessionsWithPending();
}
