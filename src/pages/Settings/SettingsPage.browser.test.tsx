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
      case "browser_extension_prepare":
        return {
          dir: "C:\\Users\\Ada\\AppData\\Local\\CodeFactory\\browser\\extension",
          port: 47615,
          token: "0123456789abcdef0123456789abcdef",
          connected: false,
          chrome_available: true,
        };
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
  it("prepares the extension itself instead of asking for a build command", async () => {
    // The regression this guards: setup used to start with `pnpm ext:build`,
    // which an installed app's user has no way to run.
    renderBrowserTab();

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith("browser_extension_prepare"),
    );
    expect(
      await screen.findByText(
        "C:\\Users\\Ada\\AppData\\Local\\CodeFactory\\browser\\extension",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/pnpm ext:build/)).not.toBeInTheDocument();
  });

  it("does not ask the user to copy a pairing code", async () => {
    // The port and token are written into the extension's folder by the app, so
    // the main flow must not present them as something to carry across.
    renderBrowserTab();
    await screen.findByRole("button", { name: "复制路径" });

    expect(screen.getByText(/不用复制任何东西/)).toBeInTheDocument();
    // Still reachable for a store install, but folded away.
    expect(screen.getByText("手动配对(一般不需要)")).toBeInTheDocument();
  });

  it("copies the folder path, which is the one value the user still needs", async () => {
    renderBrowserTab();

    await userEvent.click(await screen.findByRole("button", { name: "复制路径" }));

    expect(mocks.writeText).toHaveBeenCalledWith(
      "C:\\Users\\Ada\\AppData\\Local\\CodeFactory\\browser\\extension",
    );
    expect(await screen.findByText("已复制")).toBeInTheDocument();
  });

  it("opens Chrome's extensions page and the folder on request", async () => {
    // Both shortcuts exist so the remaining step is clicking, not navigating a
    // path under AppData by hand.
    renderBrowserTab();

    await userEvent.click(await screen.findByRole("button", { name: "打开 Chrome 扩展页" }));
    expect(mocks.invoke).toHaveBeenCalledWith("browser_open_extensions_page");

    await userEvent.click(screen.getByRole("button", { name: "打开扩展文件夹" }));
    expect(mocks.invoke).toHaveBeenCalledWith("browser_extension_reveal");
  });

  it("says what to do when there is no Chrome to open", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "browser_open_extensions_page") {
        throw new Error("No Chrome, Chromium or Edge was found to open.");
      }
      if (command === "browser_extension_prepare") {
        return {
          dir: "/home/ada/.codefactory/browser/extension",
          port: 47615,
          token: "t".repeat(32),
          connected: false,
          chrome_available: false,
        };
      }
      if (command === "browser_chromium_status") return { supported: true, installed: false };
      return [];
    });
    renderBrowserTab();

    await userEvent.click(await screen.findByRole("button", { name: "打开 Chrome 扩展页" }));

    expect(await screen.findByText(/No Chrome, Chromium or Edge was found/)).toBeInTheDocument();
    // The path is still on screen, so the user can finish by hand.
    expect(screen.getByText("/home/ada/.codefactory/browser/extension")).toBeInTheDocument();
  });

  // Slower than the rest on purpose: the panel polls every 5s because finishing
  // pairing inside Chrome emits no event here, and this asserts that real tick
  // rather than reaching into the component's timers.
  it("reports the connection state rather than leaving it ambiguous", { timeout: 15_000 }, async () => {
    renderBrowserTab();
    expect(await screen.findByText("未连接")).toBeInTheDocument();

    mocks.invoke.mockImplementation(async (command: string) =>
      command === "browser_extension_prepare"
        ? {
            dir: "/home/ada/.codefactory/browser/extension",
            port: 47615,
            token: "t".repeat(32),
            connected: true,
            chrome_available: true,
          }
        : command === "browser_chromium_status"
          ? { supported: true, installed: false }
          : [],
    );

    // The panel polls, because finishing pairing in Chrome emits no event here.
    await waitFor(() => expect(screen.getByText("已连接")).toBeInTheDocument(), {
      timeout: 8000,
    });
    // Once connected there is nothing left to set up, so the instructions go away.
    expect(screen.getByText(/重启后会自己恢复/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "复制路径" })).not.toBeInTheDocument();
  });

  it("surfaces a folder that could not be prepared", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "browser_extension_prepare") {
        throw new Error("CodeFactory could not find a writable folder for the extension");
      }
      if (command === "browser_chromium_status") return { supported: true, installed: false };
      return [];
    });
    renderBrowserTab();

    expect(await screen.findByText(/could not find a writable folder/)).toBeInTheDocument();
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
        : command === "browser_extension_prepare"
          ? {
              dir: "/home/ada/.codefactory/browser/extension",
              port: 47615,
              token: "t".repeat(32),
              connected: false,
              chrome_available: true,
            }
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
        : command === "browser_extension_prepare"
          ? {
              dir: "/home/ada/.codefactory/browser/extension",
              port: 47615,
              token: "t".repeat(32),
              connected: false,
              chrome_available: true,
            }
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
        : command === "browser_extension_prepare"
          ? {
              dir: "/home/ada/.codefactory/browser/extension",
              port: 47615,
              token: "t".repeat(32),
              connected: false,
              chrome_available: true,
            }
          : [],
    );
    renderBrowserTab();

    expect(await screen.findByText(/没有可用的 Chromium 构建/)).toBeInTheDocument();
  });

  it("surfaces a failed download instead of spinning forever", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "browser_download_chromium") throw new Error("network unreachable");
      if (command === "browser_chromium_status") return { supported: true, installed: false };
      if (command === "browser_extension_prepare")
        return {
          dir: "/home/ada/.codefactory/browser/extension",
          port: 47615,
          token: "t".repeat(32),
          connected: false,
          chrome_available: true,
        };
      return [];
    });
    renderBrowserTab();

    await userEvent.click(await screen.findByRole("button", { name: "下载浏览器" }));

    expect(await screen.findByText(/network unreachable/)).toBeInTheDocument();
    // And the button comes back, so the user can retry.
    expect(await screen.findByRole("button", { name: "下载浏览器" })).toBeInTheDocument();
  });
});
