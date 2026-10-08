// SPDX-License-Identifier: Apache-2.0
//
// A new session's first impression must not depend on where it was started
// from. Each of the three entry paths below leaves the chat store in a
// different state — a conversation that ended, a conversation with a turn still
// running, a conversation whose history has not arrived yet — and all three
// must land on the same start page.
//
// jsdom computes no layout, so the geometry assertions (input visible, opens at
// the top, no inherited offset) also run in a real browser through
// scripts/verify-new-session-start-headless.mjs. What this file pins is the
// decision itself: the start page is chosen by "nothing to show", never by a
// raw message count, and it resets its own scroll box.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

vi.mock("../lib/tauri", () => ({
  invoke: vi.fn(() => Promise.reject(new Error("usage unavailable"))),
}));
vi.mock("../stores/settings", () => ({
  useSettingsStore: () => ({ settings: null }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

import { MessageList } from "./MessageList";
import { activeRuntime, openSessionId, useChatStore } from "../stores/chat";
import type { UIMessage } from "../stores/chatEvents";

const SESSION = {
  id: "previous-session",
  title: "上一个会话",
  cwd: "/tmp/project",
  model_id: "model",
  created_at: 0,
  updated_at: 0,
  total_input_tokens: 0,
  total_output_tokens: 0,
  kind: "project",
};

const msg = (over: Partial<UIMessage> & { id: string }): UIMessage => ({
  role: "assistant",
  content: "",
  createdAt: 1,
  ...over,
});

const settledTurn = [
  msg({ id: "u1", role: "user", content: "帮我实现一个新功能" }),
  msg({
    id: "a1",
    content: "已经实现并验证。",
    segments: [{ kind: "text", text: "已经实现并验证。" }],
  }),
];

const runningTurn = [
  ...settledTurn,
  msg({
    id: "t1",
    role: "tool",
    content: "",
    toolCalls: [{ id: "tc1", name: "bash", args: "{}", status: "running" }],
  }),
];

/** Only rows the conversation cannot show: the old empty-state check passed
 *  this straight into the message branch and painted a blank page. */
const hiddenRowsOnly = [
  msg({ id: "t2", role: "tool", content: "{}" }),
  msg({ id: "s1", role: "system", content: "内部提示" }),
];

const startPagePaths: { label: string; seed: () => void }[] = [
  {
    label: "已结束的会话",
    seed: () => {
      useChatStore.setState({
        sessions: [SESSION] as never,
        activeSession: SESSION as never,
        draftSession: null,
        runtime: {
          [SESSION.id]: { messages: settledTurn, streaming: false } as never,
        },
      });
    },
  },
  {
    label: "有回合正在运行",
    seed: () => {
      useChatStore.setState({
        sessions: [SESSION] as never,
        activeSession: SESSION as never,
        draftSession: null,
        runtime: {
          [SESSION.id]: { messages: runningTurn, streaming: true } as never,
        },
      });
    },
  },
  {
    label: "历史仍在加载",
    seed: () => {
      // The session is selected but its runtime bucket has not been hydrated.
      useChatStore.setState({
        sessions: [SESSION] as never,
        activeSession: SESSION as never,
        draftSession: null,
        runtime: {},
      });
    },
  },
];

function startPageMarkup(seed: () => void): string {
  seed();
  const draft = useChatStore.getState().beginDraft();
  const state = useChatStore.getState();
  expect(openSessionId(state)).toBe(draft.id);
  const { container, unmount } = render(
    <MessageList
      messages={activeRuntime(state).messages}
      streaming={false}
      turnActive={false}
      cwd={null}
      conversationKey={openSessionId(state)}
    />,
  );
  const markup = container.innerHTML;
  expect(container.textContent).toContain("CodeFactory");
  expect(container.textContent).toContain("可以试试");
  expect(
    container.querySelector('[data-testid="conversation-reading-column"]'),
    "a start page must not render the conversation reading column",
  ).toBeNull();
  unmount();
  return markup;
}

describe("new session start page", () => {
  beforeEach(() => {
    useChatStore.setState({
      sessions: [],
      activeSession: null,
      draftSession: null,
      runtime: {},
    });
  });

  it("renders the same start page from every entry path", () => {
    const rendered = startPagePaths.map((path) => ({
      label: path.label,
      markup: startPageMarkup(path.seed),
    }));
    for (const entry of rendered) {
      expect(entry.markup, `${entry.label} must render the start page`).not.toBe("");
    }
    const [first, ...rest] = rendered;
    for (const entry of rest) {
      expect(entry.markup, `${entry.label} differs from ${first.label}`).toBe(
        first.markup,
      );
    }
  });

  it("renders the start page for a history that has no displayable rows", () => {
    useChatStore.setState({
      sessions: [SESSION] as never,
      activeSession: SESSION as never,
      draftSession: null,
      runtime: {
        [SESSION.id]: { messages: hiddenRowsOnly, streaming: false } as never,
      },
    });
    const draft = useChatStore.getState().beginDraft();
    const { container } = render(
      <MessageList
        messages={hiddenRowsOnly}
        streaming={false}
        turnActive={false}
        cwd={null}
        conversationKey={draft.id}
      />,
    );
    expect(container.textContent).toContain("CodeFactory");
  });

  it("opens at its own top instead of inheriting the previous scroll offset", () => {
    useChatStore.getState().beginDraft();
    useChatStore.setState({
      sessions: [SESSION] as never,
      activeSession: SESSION as never,
      draftSession: null,
      runtime: {
        [SESSION.id]: { messages: settledTurn, streaming: false } as never,
      },
    });
    const state = useChatStore.getState();
    const view = render(
      <MessageList
        messages={activeRuntime(state).messages}
        streaming={false}
        cwd={null}
        conversationKey={openSessionId(state)}
      />,
    );
    const scroller = view.container.querySelector(
      ".relative > .absolute.inset-0.overflow-y-auto",
    ) as HTMLElement;
    expect(scroller).not.toBeNull();

    // The conversation left the shared scroll box far from its top.
    const written: number[] = [];
    Object.defineProperty(scroller, "scrollTop", {
      configurable: true,
      get: () => 640,
      set: (value: number) => written.push(value),
    });

    const draft = useChatStore.getState().beginDraft();
    view.rerender(
      <MessageList
        messages={activeRuntime(useChatStore.getState()).messages}
        streaming={false}
        cwd={null}
        conversationKey={draft.id}
      />,
    );

    expect(scroller.isConnected, "the scroll box is reused across the swap").toBe(
      true,
    );
    expect(written, "the start page must reset itself to the top").toContain(0);
    expect(written, "the start page must not keep the old offset").not.toContain(
      640,
    );
  });
});
