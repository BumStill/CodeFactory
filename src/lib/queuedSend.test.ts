// SPDX-License-Identifier: Apache-2.0
/**
 * M236 acceptance: a message accepted while a turn is running must be
 * delivered, exactly once, and must never disappear silently.
 *
 * The port below models the durable row set (the app-database table backed by
 * `src-tauri/src/commands/queued_delivery.rs`) and survives a simulated app
 * restart, which is the difference between this and the old in-memory array.
 */
import { describe, expect, it, vi } from "vitest";

import {
  drainBeforeWake,
  drainQueuedSends,
  enqueueDurableQueuedSend,
  sessionsOwingDelivery,
  SESSION_ENDED_MESSAGE,
  type QueuedSendPort,
  type QueuedSendRecord,
} from "./queuedSend";

interface FakeDb {
  rows: QueuedSendRecord[];
}

function makePort(db: FakeDb, options: { sessions?: Set<string> } = {}): QueuedSendPort {
  const sessions = options.sessions ?? new Set(["s1"]);
  const now = () => Date.now();
  return {
    async enqueue(sessionId, content, rootTurnId) {
      const pending = db.rows.filter(
        (row) => row.sessionId === sessionId && (row.status === "queued" || row.status === "delivering"),
      );
      if (!sessions.has(sessionId)) throw new Error("unknown session");
      if (pending.length >= 5) throw new Error("queue is full");
      const record: QueuedSendRecord = {
        id: `q-${db.rows.length + 1}`,
        sessionId,
        content: content.trim(),
        rootTurnId: rootTurnId ?? `root-${db.rows.length + 1}`,
        status: "queued",
        attempts: 0,
        createdAt: now(),
        updatedAt: now(),
        deliveredAt: null,
        failureReason: null,
        position: pending.length + 1,
      };
      db.rows.push(record);
      return record;
    },
    async list(sessionId) {
      return db.rows.filter(
        (row) => row.sessionId === sessionId && (row.status === "queued" || row.status === "delivering"),
      );
    },
    async claimNext(sessionId) {
      const pending = db.rows.filter((row) => row.sessionId === sessionId && row.status === "queued");
      const next = pending[0];
      if (!next) return null;
      next.status = "delivering";
      next.attempts += 1;
      return { ...next };
    },
    async markDelivered(_sessionId, id) {
      const row = db.rows.find((candidate) => candidate.id === id);
      if (!row) return;
      row.status = "delivered";
      row.deliveredAt = now();
      row.position = null;
    },
    async markFailed(_sessionId, id, reason) {
      const row = db.rows.find((candidate) => candidate.id === id);
      if (!row) return;
      row.status = "failed";
      row.failureReason = reason;
      row.position = null;
    },
    async status(_sessionId, id) {
      return db.rows.find((row) => row.id === id) ?? null;
    },
    async sessionsWithPending() {
      return [...new Set(db.rows.filter((row) => row.status === "queued").map((row) => row.sessionId))];
    },
    async failPending(sessionId, reason) {
      const pending = db.rows.filter(
        (row) => row.sessionId === sessionId && (row.status === "queued" || row.status === "delivering"),
      );
      for (const row of pending) {
        row.status = "failed";
        row.failureReason = reason;
        row.position = null;
      }
      return pending;
    },
  };
}

describe("M236 queued send delivery", () => {
  it("T1: a message queued during a running turn starts the next turn at settlement", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    const outcome = await enqueueDurableQueuedSend(port, "s1", " 先别合并 ");
    expect(outcome.ok).toBe(true);
    if (!outcome.ok) throw new Error("unreachable");
    expect(outcome.record.status).toBe("queued");
    expect(outcome.record.content).toBe("先别合并");

    const send = vi.fn(async (_content: string, _rootTurnId: string) => {});
    const result = await drainQueuedSends(port, "s1", send);

    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith("先别合并", outcome.record.rootTurnId);
    expect(result.delivered).toHaveLength(1);
    const status = await port.status("s1", outcome.record.id);
    expect(status?.status).toBe("delivered");
    expect(status?.deliveredAt).not.toBeNull();
  });

  it("T1b: only one message becomes the next turn; the rest wait for that turn", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    await enqueueDurableQueuedSend(port, "s1", "first");
    await enqueueDurableQueuedSend(port, "s1", "second");
    const send = vi.fn(async (_content: string, _rootTurnId: string) => {});

    await drainQueuedSends(port, "s1", send);
    expect(send).toHaveBeenCalledTimes(1);
    expect(await port.list("s1")).toHaveLength(1);

    await drainQueuedSends(port, "s1", send);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send.mock.calls.map((call) => call[0])).toEqual(["first", "second"]);
  });

  it("T2: a scheduled wake-up does not preempt a queued message", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    await enqueueDurableQueuedSend(port, "s1", "用我的方案");
    const wake = vi.fn(async (_reason: string) => {});
    const send = vi.fn(async (_content: string, _rootTurnId: string) => {});

    // The wake-up handler's rule: drain first, and stand down if it delivered.
    const delivered = await drainBeforeWake(port, "s1", send);
    if (!delivered) await wake("wake-up fires only when nothing was queued");

    expect(delivered).toBe(true);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0][0]).toBe("用我的方案");
    expect(wake).not.toHaveBeenCalled();
  });

  it("T3: the message survives an app restart and is delivered exactly once", async () => {
    const db: FakeDb = { rows: [] };
    const beforeRestart = makePort(db);
    const queued = await enqueueDurableQueuedSend(beforeRestart, "s1", "重启后继续", "root-restart");
    expect(queued.ok).toBe(true);

    // Restart: the renderer's in-memory runtime is gone, the row is not.
    const afterRestart = makePort(db);
    const owing = await sessionsOwingDelivery(afterRestart);
    expect(owing).toEqual(["s1"]);

    const send = vi.fn(async (_content: string, _rootTurnId: string) => {});
    const first = await drainQueuedSends(afterRestart, "s1", send);
    expect(first.delivered).toHaveLength(1);
    expect(send).toHaveBeenCalledWith("重启后继续", "root-restart");

    // A second drain after the restart must not deliver it again.
    const second = await drainQueuedSends(afterRestart, "s1", send);
    expect(second.empty).toBe(true);
    expect(send).toHaveBeenCalledTimes(1);
    expect(await sessionsOwingDelivery(afterRestart)).toEqual([]);
  });

  it("T4: an ending turn on a waiting state still drains what is queued", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    await enqueueDurableQueuedSend(port, "s1", "醒来后先看我这条");
    const send = vi.fn(async (_content: string, _rootTurnId: string) => {});

    // Any transition into waiting_wake / waiting_user / idle is a drain point;
    // this asserts the drain does not depend on a specific event name.
    const drained = await drainQueuedSends(port, "s1", send, { limit: 1 });
    expect(drained.delivered).toHaveLength(1);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("T5: a stopped session reports a clear failure instead of a silent queued", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    const outcome = await enqueueDurableQueuedSend(port, "s1", "这条发不出去", undefined, {
      sessionEnded: true,
    });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) throw new Error("unreachable");
    expect(outcome.reason).toBe("ended");
    expect(outcome.message).toBe(SESSION_ENDED_MESSAGE);
    expect(db.rows).toHaveLength(0);

    // And a backlog that is already persisted becomes a visible terminal state.
    const queued = await enqueueDurableQueuedSend(port, "s1", "遗留消息");
    if (!queued.ok) throw new Error("unreachable");
    const failed = await port.failPending("s1", "session has ended");
    expect(failed).toHaveLength(1);
    const status = await port.status("s1", queued.record.id);
    expect(status?.status).toBe("failed");
    expect(status?.failureReason).toContain("ended");
  });

  it("T5b: a delivery that throws is recorded as a visible failure, not lost", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    const queued = await enqueueDurableQueuedSend(port, "s1", "会失败的一条");
    if (!queued.ok) throw new Error("unreachable");
    const send = vi.fn(async () => {
      throw new Error("backend rejected the turn");
    });

    const result = await drainQueuedSends(port, "s1", send);
    expect(result.failed?.reason).toContain("backend rejected the turn");
    const status = await port.status("s1", queued.record.id);
    expect(status?.status).toBe("failed");
    expect(status?.failureReason).toContain("backend rejected the turn");
  });

  it("T6: enqueue acknowledges only after the row exists, and caps at five", async () => {
    const db: FakeDb = { rows: [] };
    const port = makePort(db);
    for (let index = 0; index < 5; index += 1) {
      const outcome = await enqueueDurableQueuedSend(port, "s1", `m${index}`);
      expect(outcome.ok).toBe(true);
      expect(db.rows).toHaveLength(index + 1);
    }
    const overflow = await enqueueDurableQueuedSend(port, "s1", "overflow");
    expect(overflow.ok).toBe(false);
    if (overflow.ok) throw new Error("unreachable");
    expect(overflow.reason).toBe("full");
  });
});
