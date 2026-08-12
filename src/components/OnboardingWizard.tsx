// SPDX-License-Identifier: Apache-2.0
//
// First-run readiness wizard — a NON-BLOCKING corner card (the earlier
// full-screen overlay was removed precisely because it hid the workspace).
// Three checks, each green when already satisfied:
//   1. model access, 2. delivery channel (logged-in gh preferred, zero
//   app-side config), 3. delivery ceiling choice.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, X } from "lucide-react";
import type { Settings } from "../lib/tauri";

interface ChannelStatus {
  gh_cli: boolean;
  rest_token: boolean;
}

export function OnboardingWizard({
  modelReady,
  ceiling,
  onCeilingChange,
  onDone,
}: {
  modelReady: boolean;
  ceiling: NonNullable<Settings["delivery_ceiling"]>;
  onCeilingChange: (ceiling: NonNullable<Settings["delivery_ceiling"]>) => void;
  onDone: () => void;
}) {
  const [channel, setChannel] = useState<ChannelStatus | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    invoke<ChannelStatus>("delivery_channel_status")
      .then((status) => {
        if (!cancelled) setChannel(status);
      })
      .catch(() => {
        if (!cancelled) setChannel({ gh_cli: false, rest_token: false });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const copyGhLogin = () => {
    navigator.clipboard.writeText("gh auth login").then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <div
      data-testid="onboarding-wizard"
      className="fixed bottom-4 right-4 z-40 w-80 rounded-lg border border-border bg-surface-1 p-3.5 shadow-xl space-y-2.5"
    >
      <div className="flex items-center justify-between">
        <span className="text-label font-semibold text-gray-300">快速就绪检查</span>
        <button
          type="button"
          onClick={onDone}
          aria-label="关闭引导"
          className="p-0.5 rounded text-gray-500 hover:text-gray-300"
        >
          <X size={14} />
        </button>
      </div>

      <div className="space-y-1.5 text-label leading-5">
        <div className="flex items-start gap-1.5">
          {modelReady ? (
            <>
              <Check size={14} className="mt-0.5 text-emerald-500 shrink-0" />
              <span className="text-gray-300">模型已接入,可以直接对话。</span>
            </>
          ) : (
            <span className="text-gray-400">
              ① 还没有可用模型:在 <span className="text-gray-200">设置 → 端点</span> 填入
              API Key,或用 ChatGPT 登录(非官方通道,可能失效)。
            </span>
          )}
        </div>

        <div className="flex items-start gap-1.5">
          {channel === null ? (
            <span className="text-gray-500">② 正在检测交付通道…</span>
          ) : channel.gh_cli ? (
            <>
              <Check size={14} className="mt-0.5 text-emerald-500 shrink-0" />
              <span className="text-gray-300">
                GitHub CLI 已就绪——GitHub/GHE 的 PR/CI/合并/发布可零配置使用；其他 Git 平台可用远程令牌或 delivery_provider hook。
              </span>
            </>
          ) : channel.rest_token ? (
            <>
              <Check size={14} className="mt-0.5 text-emerald-500 shrink-0" />
              <span className="text-gray-300">已配置远端令牌,交付链可用。</span>
            </>
          ) : (
            <span className="text-gray-400">
              ② GitHub/GHE 交付可在终端执行
              <button
                type="button"
                onClick={copyGhLogin}
                className="mx-1 inline-flex items-center gap-1 rounded border border-border bg-surface-2 px-1.5 py-0.5 font-mono text-caption text-gray-200"
              >
                gh auth login
                {copied ? <Check size={14} className="text-emerald-500" /> : <Copy size={14} />}
              </button>
              登录一次即可；GitLab/Bitbucket/Azure/Gitea/Forgejo/Gerrit/Zeabur 等请在设置 → 远程仓库配置令牌或 delivery_provider hook。
            </span>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          <span className="text-gray-400">③ 自动交付到:</span>
          <select
            value={ceiling}
            onChange={(e) =>
              onCeilingChange(e.target.value as NonNullable<Settings["delivery_ceiling"]>)
            }
            className="rounded border border-border bg-surface-2 px-1.5 py-0.5 text-caption text-gray-300"
          >
            <option value="pr_only">开 PR 为止</option>
            <option value="through_ci_green">等 CI 通过</option>
            <option value="through_merge">合并</option>
            <option value="through_release">创建正式发布(默认)</option>
          </select>
        </div>
      </div>

      <div className="flex justify-end gap-2 pt-0.5">
        <button
          type="button"
          onClick={onDone}
          className="rounded border border-border px-2.5 py-1 text-caption text-gray-400 hover:text-gray-200"
        >
          跳过
        </button>
        <button
          type="button"
          onClick={onDone}
          className="rounded bg-accent px-2.5 py-1 text-caption text-white hover:bg-accent-hover"
        >
          完成
        </button>
      </div>
    </div>
  );
}
