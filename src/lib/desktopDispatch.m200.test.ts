// SPDX-License-Identifier: Apache-2.0
import { expect, it, vi } from "vitest";
import { applyDispatchRequest } from "./desktopDispatch";

it("M200 R3 client_request_id survives request translation", async () => {
  const createAndSend = vi.fn(async () => ({ session_id: "synthetic" }));
  const handlers = { createAndSend } as unknown as Parameters<typeof applyDispatchRequest>[1];
  const event = { request_id: "transport-1", operation: "create_and_send", request: { project: "/synthetic", message: "synthetic task", delivery_authorized: false, client_request_id: "synthetic-key" } };
  expect((await applyDispatchRequest(event, handlers)).ok).toBe(true);
  expect(createAndSend).toHaveBeenCalledWith(expect.objectContaining({ clientRequestId: "synthetic-key" }));
});
