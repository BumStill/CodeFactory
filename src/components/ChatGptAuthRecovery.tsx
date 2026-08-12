// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import {
  codexLoginCancel,
  codexLoginOpen,
  codexLoginStart,
  codexLoginStatus,
  type CodexLoginFlow,
} from "../lib/tauri";

export function ChatGptAuthRecovery() {
  const [flow, setFlow] = useState<CodexLoginFlow | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!flow || (flow.status !== "waiting" && flow.status !== "exchanging")) return;
    let stopped = false;
    const timer = window.setInterval(() => {
      void codexLoginStatus(flow.flow_id)
        .then((next) => {
          if (!stopped) setFlow(next);
        })
        .catch((pollError) => {
          if (!stopped) {
            setError(pollError instanceof Error ? pollError.message : String(pollError));
          }
        });
    }, 800);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [flow?.flow_id, flow?.status]);

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      setFlow(await codexLoginStart());
    } catch (startError) {
      setError(startError instanceof Error ? startError.message : String(startError));
    } finally {
      setBusy(false);
    }
  };

  if (!flow || ["failed", "cancelled", "expired"].includes(flow.status)) {
    return (
      <div className="space-y-1.5">
        <button
          type="button"
          onClick={() => void start()}
          disabled={busy}
          className="rounded bg-accent px-2.5 py-1.5 text-label text-white disabled:opacity-50"
        >
          {busy ? "正在准备验证…" : "重新验证"}
        </button>
        {(error || flow?.error_message) && (
          <p className="max-w-[72ch] text-label leading-5 text-rose-500">
            {error ?? flow?.error_message}
          </p>
        )}
      </div>
    );
  }

  if (flow.status === "succeeded") {
    return (
      <p role="status" className="text-label leading-5 text-emerald-700 dark:text-emerald-300">
        ChatGPT 已重新连接。为避免重复工具或副作用，请在输入框中明确重新发送需要继续的内容。
      </p>
    );
  }

  return (
    <div role="status" aria-live="polite" className="space-y-2">
      <p className="text-label leading-5 text-gray-500">
        {flow.status === "exchanging"
          ? "正在完成验证…"
          : "若浏览器没有自动打开，可手动打开或复制同一条验证链接。"}
      </p>
      {flow.browser_open_error && (
        <p className="text-label leading-5 text-amber-700 dark:text-amber-300">
          自动打开失败：{flow.browser_open_error}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <button
          type="button"
          disabled={flow.status === "exchanging"}
          onClick={() => {
            setError(null);
            void codexLoginOpen(flow.flow_id)
              .then((next) => {
                setFlow(next);
                if (next.browser_open_error) setError(next.browser_open_error);
              })
              .catch((openError) =>
                setError(openError instanceof Error ? openError.message : String(openError))
              );
          }}
          className="rounded bg-accent px-2.5 py-1.5 text-label text-white disabled:opacity-50"
        >
          打开验证页面
        </button>
        <button
          type="button"
          onClick={() => {
            void navigator.clipboard
              .writeText(flow.authorization_url)
              .then(() => {
                setCopied(true);
                window.setTimeout(() => setCopied(false), 1500);
              })
              .catch((copyError) =>
                setError(copyError instanceof Error ? copyError.message : "复制验证链接失败")
              );
          }}
          className="rounded border border-border px-2.5 py-1.5 text-label text-gray-300 hover:bg-surface-3"
        >
          {copied ? "已复制" : "复制链接"}
        </button>
        <button
          type="button"
          disabled={busy || flow.status === "exchanging"}
          onClick={() => {
            setBusy(true);
            void codexLoginCancel(flow.flow_id)
              .then(setFlow)
              .catch((cancelError) =>
                setError(cancelError instanceof Error ? cancelError.message : String(cancelError))
              )
              .finally(() => setBusy(false));
          }}
          className="rounded px-2.5 py-1.5 text-label text-gray-500 hover:bg-surface-3 hover:text-gray-300 disabled:opacity-50"
        >
          取消
        </button>
      </div>
      {error && <p className="max-w-[72ch] text-label leading-5 text-rose-500">{error}</p>}
    </div>
  );
}
