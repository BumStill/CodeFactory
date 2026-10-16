// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for the usage-window settings section plus the
// M86 follow-up: CF-WINDOW-R5 (the wait sentence must show up *in the session*)
// and CF-WINDOW-R6 (the prompt is an in-app dialog, never window.alert/confirm).
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import "../styles/globals.css";
import { useSettingsStore } from "../stores/settings";
import type { Settings } from "../lib/tauri";
import { ModelUsageWindowsSection } from "../components/ModelUsageWindowsSection";
import { UsageWindowDialog } from "../components/UsageWindowDialog";
import { MessageList } from "../components/MessageList";
import { USAGE_WINDOW_WAIT_LABEL, type UIMessage } from "../stores/chatEvents";
import { applyPreset, modelWindowKey } from "../lib/usageWindows";
const theme = new URLSearchParams(location.search).get("theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;
const settings = { theme, font_size:14, endpoints:{deepseek:{base_url:"https://synthetic.example",api_style:"openai",active_model:"pro",custom_models:[{id:"pro"},{id:"flash"}]}}, model_usage_windows:{[modelWindowKey("deepseek","pro")]:applyPreset("deepseek_off_peak")}, subscription_quota_caps:{} } as unknown as Settings;
mockIPC((command, args) => command === "save_settings" ? (args as { newSettings: Settings }).newSettings : settings);
useSettingsStore.setState({settings});

// CF-WINDOW-R5 / M86：GPT 已达订阅上限（14:05 重置）、DeepSeek 高峰（12:00 可用）、
// OpenRouter 未配置凭据——全部不可用，所以这是"可理解的等待"，不是失败。
const waitNotice =
  "等待模型可用：ChatGPT 已达订阅用量上限，10-14 14:05 重置；DeepSeek 高峰时段，10-14 12:00 可用；OpenRouter 尚未配置凭据；最早 10-14 12:00 自动继续。等待不计入技术恢复次数；可在设置里放开时段，或换一个模型";

const waitMessage: UIMessage = {
  id: "usage-window-wait-acceptance",
  role: "assistant",
  content: "",
  createdAt: Date.now() - 5_000,
  transportRetries: [
    { label: USAGE_WINDOW_WAIT_LABEL, attempt: 0, maxAttempts: 0, delayMs: 1000, reason: waitNotice },
  ],
};

const nextTransition = Date.now() + 3 * 60 * 60 * 1000;

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mt-6 rounded-lg border border-border bg-surface-1 p-3">
      <h2 className="mb-3 text-body font-semibold text-gray-200">{title}</h2>
      {children}
    </section>
  );
}

createRoot(document.getElementById("root")!).render(
  <main className="min-h-screen bg-surface-0 p-4">
    <ModelUsageWindowsSection />
    <Panel title="CF-WINDOW-R5：所有模型都不可用时的等待说明（会话内可见）">
      <div className="rounded border border-border bg-bg p-2">
        <MessageList
          messages={[waitMessage]}
          streaming
          cwd={null}
          conversationKey="usage-window-wait-acceptance"
        />
      </div>
    </Panel>
    <Panel title="CF-WINDOW-R6：应用内对话框（替代原生弹窗）">
      <div className="relative h-[320px] overflow-hidden rounded border border-border bg-bg">
        <UsageWindowDialog
          embedded
          endpoint="chatgpt"
          model="gpt-5.5"
          nextTransitionMs={nextTransition}
          onIgnoreForSession={() => {}}
          onCancel={() => {}}
        />
      </div>
    </Panel>
  </main>,
);
