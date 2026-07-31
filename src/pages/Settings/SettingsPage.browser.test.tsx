// SPDX-License-Identifier: Apache-2.0
//
// The two panels on the Browser tab. What can actually break here is the data
// flow — a wrong command name, a field read from the wrong place, a progress
// event that never reaches the bar — so these tests drive the real components
// against a stubbed Tauri bridge and assert what the user ends up seeing.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  writeText: vi.fn(),
}));

vi.mock("../../lib/tauri", async (orig) => ({
  ...((await orig()) as object),
  invoke: mocks.invoke,
  onChromiumProgress: mocks.listen,
}));

// Reuses the settings shape the other SettingsPage tests use: the page early
// returns a loading state until settings resolve, so a null fixture renders
// nothing at all and every assertion would fail for the wrong reason.
const settingsState = vi.hoisted(() => ({
  settings: {
    endpoints: {
      chatgpt: {
        base_url: "https://chatgpt.com/backend-api/codex",
        api_style: "chatgpt" as const,
        custom_models: [],
        active_model: "gpt-5.6-sol",
      },
    },
    default_endpoint: "chatgpt",
    default_model: "gpt-5.6-sol",
    permissions: { allow: [], ask: [], deny: [], full_access: true },
    shell: { shell: "zsh" },
    auto_create_pr: false,
    theme: "dark" as const,
    font_family: "inter",
    font_size: 14,
    reasoning_effort: "medium" as const,
    onboarded: true,
  },
  load: vi.fn(),
  save: vi.fn(),
  setTheme: vi.fn(),
}));

vi.mock("../../stores/settings", () => {
  function useSettingsStore<T>(selector?: (state: typeof settingsState) => T) {
    return selector ? selector(settingsState) : settingsState;
  }
  useSettingsStore.getState = () => settingsState;
  return { useSettingsStore };
});
vi.mock("../../stores/chat", () => {
  const chatState = { loadModels: vi.fn() };
  function useChatStore() {
    return chatState;
  }
  useChatStore.getState = () => chatState;
  return { useChatStore };
});
vi.mock("../../stores/gitRemote", () => ({
  useGitRemoteStore: () => ({
    remotes: [],
    loadRemotes: vi.fn(),
    addRemote: vi.fn(),
    deleteRemote: vi.fn(),
    testRemote: vi.fn(),
  }),
}));
vi.mock("../../stores/updater", () => ({
  useUpdaterStore: () => ({
    phase: { kind: "idle" as const },
    currentVersion: "dev",
    initialize: vi.fn(),
    checkNow: vi.fn(),
    install: vi.fn(),
  }),
}));

import { SettingsPage } from "./SettingsPage";

/** Emitted progress handlers, so a test can drive the download bar. */
let emit: ((payload: Record<string, unknown>) => void) | null = null;

function renderBrowserTab() {
  return render(
    <SettingsPage
      onBack={() => {}}
      initialTab="browser"
      onOpenSession={() => {}}
      onOpenJobLog={() => {}}
      onOpenResources={() => {}}
      onOpenControlPlane={() => {}}
      onOpenBenchmarks={() => {}}
      onOpenProfile={() => {}}
      onOpenEvolution={() => {}}
    />,
  );
}

beforeEach(() => {
  mocks.invoke.mockReset();
  mocks.listen.mockReset();
  mocks.writeText.mockReset();
  emit = null;

  mocks.listen.mockImplementation(async (handler: (progress: unknown) => void) => {
    emit = (payload) => handler(payload);
    return () => {};
  });
  Object.assign(navigator, { clipboard: { writeText: mocks.writeText } });

  mocks.invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "browser_bridge_pairing":
        return { port: 51789, token: "0123456789abcdef0123456789abcdef", connected: false };
      case "browser_chromium_status":
        return { supported: true, installed: false };
      case "list_browser_sessions":
        return [];
      case "browser_download_chromium":
        return { version: "151.0.7922.71" };
      default:
        return null;
    }
  });
});

describe("browser bridge pairing panel", () => {
  it("shows the port and token the extension needs", async () => {
    renderBrowserTab();

    expect(await screen.findByText("51789")).toBeInTheDocument();
    expect(
      screen.getByText("0123456789abcdef0123456789abcdef"),
    ).toBeInTheDocument();
  });

  it("copies a value to the clipboard and confirms it", async () => {
    // The whole panel exists because the token has to cross to the browser by
    // hand; a copy button that silently does nothing would be the worst bug.
    renderBrowserTab();
    await screen.findByText("51789");

    await userEvent.click(screen.getAllByRole("button", { name: "复制" })[0]);

    expect(mocks.writeText).toHaveBeenCalledWith("51789");
    expect(await screen.findByText("已复制")).toBeInTheDocument();
  });

  // Slower than the rest on purpose: the panel polls every 5s because finishing
  // pairing inside Chrome emits no event here, and this asserts that real tick
  // rather than reaching into the component's timers.
  it("reports the connection state rather than leaving it ambiguous", { timeout: 15_000 }, async () => {
    renderBrowserTab();
    expect(await screen.findByText("未连接")).toBeInTheDocument();

    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_bridge_pairing"
        ? { port: 51789, token: "t".repeat(32), connected: true }
        : command === "browser_chromium_status"
          ? { supported: true, installed: false }
          : [],
    );

    // The panel polls, because finishing pairing in Chrome emits no event here.
    await waitFor(() => expect(screen.getByText("已连接")).toBeInTheDocument(), {
      timeout: 8000,
    });
  });

  it("spells out the install steps, since they cannot be automated", async () => {
    renderBrowserTab();
    await screen.findByText("51789");

    expect(screen.getByText(/pnpm ext:build/)).toBeInTheDocument();
    expect(screen.getByText(/chrome:\/\/extensions/)).toBeInTheDocument();
    expect(screen.getByText(/extension\/dist/)).toBeInTheDocument();
  });
});

describe("managed chromium panel", () => {
  it("offers the download when nothing is installed", async () => {
    renderBrowserTab();
    expect(await screen.findByRole("button", { name: "下载浏览器" })).toBeInTheDocument();
    expect(screen.getByText(/约 150 MB/)).toBeInTheDocument();
  });

  it("turns progress events into visible progress", async () => {
    // A 150 MB download with no feedback reads as a frozen app.
    renderBrowserTab();
    await userEvent.click(await screen.findByRole("button", { name: "下载浏览器" }));

    emit?.({ stage: "downloading", received_bytes: 45_000_000, total_bytes: 150_000_000 });
    expect(await screen.findByText("正在下载 45 MB / 150 MB")).toBeInTheDocument();

    emit?.({ stage: "extracting" });
    expect(await screen.findByText("正在解压…")).toBeInTheDocument();
  });

  it("falls back to a byte count when the server sends no total", async () => {
    renderBrowserTab();
    await userEvent.click(await screen.findByRole("button", { name: "下载浏览器" }));

    emit?.({ stage: "downloading", received_bytes: 12_000_000, total_bytes: null });
    expect(await screen.findByText("正在下载 12 MB")).toBeInTheDocument();
  });

  it("shows the version once installed instead of offering the download again", async () => {
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_chromium_status"
        ? { supported: true, installed: true, version: "151.0.7922.71" }
        : command === "browser_bridge_pairing"
          ? { port: 1, token: "t".repeat(32), connected: false }
          : [],
    );
    renderBrowserTab();

    expect(await screen.findByText("已就绪")).toBeInTheDocument();
    expect(screen.getByText("版本 151.0.7922.71")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载浏览器" })).not.toBeInTheDocument();
  });

  it("offers a repair rather than a plain download after a broken install", async () => {
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_chromium_status"
        ? { supported: true, installed: false, needs_repair: true }
        : command === "browser_bridge_pairing"
          ? { port: 1, token: "t".repeat(32), connected: false }
          : [],
    );
    renderBrowserTab();

    expect(
      await screen.findByRole("button", { name: "重新下载(修复)" }),
    ).toBeInTheDocument();
  });

  it("says so plainly on a platform with no build", async () => {
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_chromium_status"
        ? { supported: false }
        : command === "browser_bridge_pairing"
          ? { port: 1, token: "t".repeat(32), connected: false }
          : [],
    );
    renderBrowserTab();

    expect(await screen.findByText(/没有可用的 Chromium 构建/)).toBeInTheDocument();
  });

  it("surfaces a failed download instead of spinning forever", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "browser_download_chromium") throw new Error("network unreachable");
      if (command === "browser_chromium_status") return { supported: true, installed: false };
      if (command === "browser_bridge_pairing")
        return { port: 1, token: "t".repeat(32), connected: false };
      return [];
    });
    renderBrowserTab();

    await userEvent.click(await screen.findByRole("button", { name: "下载浏览器" }));

    expect(await screen.findByText(/network unreachable/)).toBeInTheDocument();
    // And the button comes back, so the user can retry.
    expect(await screen.findByRole("button", { name: "下载浏览器" })).toBeInTheDocument();
  });
});
