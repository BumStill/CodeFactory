// SPDX-License-Identifier: Apache-2.0
// Real-browser acceptance entry for image preview rendering and lightbox behavior.

import { createRoot } from "react-dom/client";

import "../styles/globals.css";
import { MessageList } from "../components/MessageList";
import { MessageInput } from "../components/MessageInput";
import type { UIMessage } from "../stores/chatEvents";

const pngBytes = new Uint8Array([
  0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
  0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
  0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
  0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
  0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
  0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
]);
const blob = new Blob([pngBytes], { type: "image/png" });
const imageUrl = URL.createObjectURL(blob);

const messages: UIMessage[] = [
  {
    id: "m1",
    role: "user",
    content: "会话信息里的图片：\n\n![IMG_6190.png](file:///Users/leo/Projects/AI foundation/.codefactory/attachments/1785309543980-84d170b1.png)",
    createdAt: Date.now(),
  },
];

(window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {
  invoke: async (_cmd: string, args?: { message?: { cmd?: string; payload?: Record<string, unknown> } }) => {
    const cmd = args?.message?.cmd ?? _cmd;
    switch (cmd) {
      case "save_chat_attachment":
        return { path: "/Users/leo/Projects/AI foundation/.codefactory/attachments/input preview.png", name: "input preview.png", size_bytes: pngBytes.length };
      case "plugin:event|listen": return 1;
      case "plugin:event|unlisten": return null;
      default:
        return null;
    }
  },
  convertFileSrc: (path: string) => `${imageUrl}#${encodeURIComponent(path)}`,
  transformCallback: () => 1,
};
(window as typeof window & { __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: () => {},
};

function ImagePreviewAcceptance() {
  return (
    <main aria-label="Image preview acceptance" className="flex h-screen flex-col bg-surface-0 text-gray-200">
      <div className="min-h-0 flex-1">
        <MessageList messages={messages} streaming={false} cwd="/Users/leo/Projects/AI foundation" />
      </div>
      <MessageInput
        onSend={() => {}}
        onCancel={() => {}}
        streaming={false}
        disabled={false}
        cwd="/Users/leo/Projects/AI foundation"
      />
      <div aria-label="Image preview probe" data-ready="true" />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<ImagePreviewAcceptance />);
