// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { allocateLoopbackPort, waitForAcceptanceDocument } from "./headless-acceptance-support.mjs";

async function listen(port, response) {
  const server = net.createServer((socket) => {
    const body = typeof response === "function" ? response() : response;
    socket.end(`HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`);
  });
  await new Promise((resolve, reject) => server.once("error", reject).listen(port, "127.0.0.1", resolve));
  return server;
}

test("allocateLoopbackPort avoids an occupied fixed acceptance port", async () => {
  const occupied = await listen(0, "occupied");
  const occupiedPort = occupied.address().port;
  const selected = await allocateLoopbackPort(occupiedPort);
  assert.notEqual(selected, occupiedPort);
  occupied.close();
});

test("waitForAcceptanceDocument rejects stale content until the expected fixture is served", async () => {
  let requests = 0;
  const server = await listen(0, () => {
    requests += 1;
    return requests < 3
      ? "<main aria-label='Other fixture'></main>"
      : "<script src='expected-fixture.tsx'></script>";
  });
  const port = server.address().port;
  const url = `http://127.0.0.1:${port}/acceptance.html`;
  const child = { exitCode: null, signalCode: null };
  await waitForAcceptanceDocument(child, url, "expected-fixture.tsx", { timeoutMs: 2_000, pollMs: 20 });
  assert.ok(requests >= 3, "stale documents must not satisfy readiness");
  server.close();
});
