// SPDX-License-Identifier: Apache-2.0
// Live affected-scenario gate probe; intentionally no runtime behavior change.
//
// Status surface, plus manual pairing as a fallback.
//
// Normally there is nothing to do here: CodeFactory writes the live port and
// pairing code into this extension's own folder, and the service worker reads
// them. The form stays for the one case that cannot work that way — a copy of
// the extension the app did not write out (a store build, or a folder the user
// moved) — so the panel leads with what is happening and keeps the inputs
// collapsed.

/** Human-readable connection state, so a failure says what to do next. */
const STATUS_TEXT = {
  connected: "已连接 — CodeFactory 可以读取你打开的页面。",
  connecting: "正在连接 CodeFactory…",
  not_paired: "还没拿到配对信息。请确认 CodeFactory 正在运行,或在下面手动填入。",
  disconnected: "未连接。请确认 CodeFactory 正在运行——它启动后这里会自动恢复。",
  standby: "待机中 — 另一个浏览器配置正在连接 CodeFactory；当前配置会定期探测并在对方退出后接管。",
  refused: "被拒绝。配对码可能已更换 — CodeFactory 运行时会自动更新,稍等一下。",
  error: "连接出错。请确认 CodeFactory 正在运行。",
};

function render(stored) {
  document.getElementById("status").textContent =
    STATUS_TEXT[stored.status] || "正在检查连接状态…";

  // Say *how* it is paired. Without this, an automatically paired extension
  // looks identical to one that was never set up, and the user reaches for the
  // form they do not need.
  const detail = document.getElementById("detail");
  if (stored.pairingSource === "packaged") {
    detail.textContent = `已由 CodeFactory 自动配对（端口 ${stored.activePort ?? "?"}）,无需手动填写。`;
  } else if (stored.pairingSource === "manual") {
    detail.textContent = `使用手动填写的配对信息（端口 ${stored.activePort ?? "?"}）。`;
  } else {
    detail.textContent = "";
  }
}

async function load() {
  const stored = await chrome.storage.local.get([
    "port",
    "token",
    "status",
    "pairingSource",
    "activePort",
  ]);
  if (stored.port) document.getElementById("port").value = stored.port;
  if (stored.token) document.getElementById("token").value = stored.token;
  // Only unfold the form when it is the thing the user actually needs.
  if (stored.status && stored.status !== "connected" && stored.pairingSource !== "packaged") {
    document.getElementById("manual").open = true;
  }
  render(stored);
}

document.getElementById("save").addEventListener("click", async () => {
  const port = Number(document.getElementById("port").value.trim());
  const token = document.getElementById("token").value.trim();

  // Validate here rather than letting a typo become a silent reconnect loop.
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    document.getElementById("status").textContent = "端口不是有效的数字。";
    return;
  }
  if (token.length < 16) {
    document.getElementById("status").textContent = "配对码看起来不完整。";
    return;
  }

  await chrome.storage.local.set({
    port,
    token,
    status: "disconnected",
    bridgeStandby: false,
  });
  document.getElementById("status").textContent = "已保存,正在连接…";
});

chrome.storage.onChanged.addListener(() => {
  void load();
});

load();
