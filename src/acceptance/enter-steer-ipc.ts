// SPDX-License-Identifier: Apache-2.0
// Synthetic IPC boundary only; production store, reducer and dispatch stay real.
const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
export async function listen(name: string, handler: (event: { payload: unknown }) => void) {
  const handlers = listeners.get(name) ?? new Set();
  handlers.add(handler);
  listeners.set(name, handlers);
  return () => { handlers.delete(handler); };
}
export async function emit(name: string, payload: unknown) {
  listeners.get(name)?.forEach((handler) => handler({ payload }));
}
export function convertFileSrc(path: string) { return path; }
export async function invoke(command: string, args: Record<string, unknown> = {}) {
  if (command === "is_chat_running") return args.sessionId !== "stalled";
  if (command === "queue_interjection" && args.sessionId === "live") {
    setTimeout(() => void emit("stream:live", { type: "steer_applied", message_id: "synthetic-applied", content: args.message }), 25);
  }
  return undefined;
}
