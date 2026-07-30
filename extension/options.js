// SPDX-License-Identifier: Apache-2.0
//
// Pairing UI. Deliberately the only place the token is entered: the extension
// cannot read files on disk, so a human carries the value across from the app
// once, and the app can revoke it by generating a new one.

/** Human-readable connection state, so a failure says what to do next. */
const STATUS_TEXT = {
  connected: "已连接 — CodeFactory 可以读取你打开的页面。",
  not_paired: "尚未配对。填入端口和配对码。",
  disconnected: "未连接。请确认 CodeFactory 正在运行。",
  refused: "被拒绝。配对码可能已更换 — 请到设置里重新复制。",
  error: "连接出错。请确认端口正确、CodeFactory 正在运行。",
};

function render(status) {
  document.getElementById("status").textContent =
    STATUS_TEXT[status] || "正在检查连接状态…";
}

async function load() {
  const stored = await chrome.storage.local.get(["port", "token", "status"]);
  if (stored.port) document.getElementById("port").value = stored.port;
  if (stored.token) document.getElementById("token").value = stored.token;
  render(stored.status);
}

document.getElementById("save").addEventListener("click", async () => {
  const port = Number(document.getElementById("port").value.trim());
  const token = document.getElementById("token").value.trim();

  // Validate here rather than letting a typo become a silent reconnect loop.
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    render("error");
    document.getElementById("status").textContent = "端口不是有效的数字。";
    return;
  }
  if (token.length < 16) {
    document.getElementById("status").textContent = "配对码看起来不完整。";
    return;
  }

  await chrome.storage.local.set({ port, token, status: "disconnected" });
  document.getElementById("status").textContent = "已保存,正在连接…";
});

chrome.storage.onChanged.addListener((changes) => {
  if (changes.status) render(changes.status.newValue);
});

load();
