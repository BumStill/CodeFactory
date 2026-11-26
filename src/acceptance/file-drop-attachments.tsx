// SPDX-License-Identifier: Apache-2.0
import { createRoot } from "react-dom/client";
import "../styles/globals.css";
import { MessageInput } from "../components/MessageInput";

const png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==";
const calls: unknown[] = [];
Object.assign(window, {
  __m289Calls: calls,
  __TAURI_INTERNALS__: {
    invoke: async (command: string, args: Record<string, unknown>) => {
      calls.push({ command, args });
      if (command === "attachment_limits") return { max_bytes: 52428800, max_mebibytes: 50, max_model_image_bytes: 8388608, max_model_image_mebibytes: 8 };
      if (command === "save_session_attachment") return { path: `/synthetic/attachments/${args.filename}`, name: args.filename, size_bytes: 4 };
      return null;
    },
    convertFileSrc: () => png,
    transformCallback: () => 1,
  },
});

function Acceptance() {
  return (
    <main aria-label="M289 file drop" className="flex h-screen flex-col bg-surface-0 p-6 text-gray-200">
      <section className="min-h-0 flex-1 rounded-2xl border border-border bg-surface-2 p-6">
        <h1 className="text-title font-semibold">拖文件到窗口任意位置</h1>
        <p className="mt-2 text-reading text-gray-400">消息区、空白区和输入区都属于同一放置区域。</p>
      </section>
      <div className="pt-4">
        <MessageInput cwd="/synthetic" sessionId="m289" onSend={() => {}} onCancel={() => {}} streaming={false} disabled={false} />
      </div>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<Acceptance />);
