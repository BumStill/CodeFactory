// SPDX-License-Identifier: Apache-2.0
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import "../styles/globals.css";
import { ContextUsageBar } from "../components/ContextUsageBar";
import { useChatStore } from "../stores/chat";

const theme = new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;

const summary = {
  input_tokens: 12_000,
  output_tokens: 3_000,
  reasoning_tokens: 0,
  cached_tokens: 0,
  requests: 4,
  actual_cost_usd: 0.0123,
  estimated_cost_usd: null,
  cost_source: "provider_actual",
};

mockIPC((command) => {
  if (command === "get_usage_dashboard") return { summary };
  if (command === "get_session_usage") return summary;
  return undefined;
});

useChatStore.setState((state) => ({
  runtime: {
    ...state.runtime,
    "m295-session": {
      ...state.runtime?.["m295-session"],
      contextUsage: { used: 42_000, limit: 100_000, maxLimit: 100_000 },
    },
  },
}));

const nativeFocus = HTMLElement.prototype.focus;
HTMLElement.prototype.focus = function focus(options?: FocusOptions) {
  nativeFocus.call(this, options);
  if (this.dataset.testid === "context-usage-detail-portal" && !options?.preventScroll) {
    // WebView2 with an occupied classic scrollbar may scroll a newly focused
    // fixed portal into view before its layout-effect position is painted.
    window.scrollBy({ top: 96, behavior: "instant" });
  }
};

createRoot(document.getElementById("root")!).render(
  <main
    aria-label="M295 用量圈定位验收"
    className="h-[140vh] min-h-[760px] overflow-y-scroll bg-surface-0 text-gray-200"
    style={{ scrollbarGutter: "stable" }}
  >
    <div className="mx-auto flex h-[720px] max-w-[720px] flex-col justify-end px-3 pb-5">
      <section className="rounded-xl border border-border bg-surface-2 p-3 shadow-xl">
        <div className="h-24 rounded-lg bg-surface-1 p-3 text-label text-gray-400">
          Windows 经典滚动条与缩放验收容器
        </div>
        <div className="mt-2 flex items-center border-t border-border pt-2">
          <span className="text-label text-gray-500">合成会话</span>
          <div className="ml-auto shrink-0">
            <ContextUsageBar sessionId="m295-session" />
          </div>
        </div>
      </section>
    </div>
  </main>,
);
