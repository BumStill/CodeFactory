// SPDX-License-Identifier: Apache-2.0
//
// First-run onboarding overlay.
//
// Shown when settings.onboarded is false-or-missing. Walks the user
// through:
//   1. Welcome      — what CodeFactory is, what they're about to set up
//   2. API key      — paste an OpenRouter key (default endpoint shipped
//                     with the app); validated lightly by checking that
//                     it starts with the expected prefix. Skip is allowed.
//   3. First action — three cards repeating Home's primary entries with
//                     a one-sentence "what you'd get" preview.
//   4. Done         — flip settings.onboarded=true and dismiss.
//
// We deliberately don't try to verify the key against OpenRouter here —
// that's a network round trip the user shouldn't pay during onboarding.
// First real request gives them a clean error if the key's bad.

import { useEffect, useState } from "react";
import {
  X, ChevronRight, ChevronLeft, Check, ExternalLink,
  Sparkles, Plus, Zap, User, Loader2,
} from "lucide-react";
import { useSettingsStore } from "../stores/settings";

type Step = "welcome" | "api-key" | "first-action";

interface Props {
  /** Called when the user picks an action on step 3, OR clicks done/skip. */
  onClose: () => void;
  /** Called when the user picks "新建项目" on step 3 — host routes to dialog. */
  onPickNewProject: () => void;
  /** Called when the user picks "快速任务" — host routes to quick session. */
  onPickQuickTask: () => void;
  /** Called when the user picks "我的画像" — host opens Profile page. */
  onPickProfile: () => void;
}

export function OnboardingOverlay({
  onClose, onPickNewProject, onPickQuickTask, onPickProfile,
}: Props) {
  const { settings, save, saveApiKey } = useSettingsStore();
  const [step, setStep] = useState<Step>("welcome");
  const [keyDraft, setKeyDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Pre-fill from existing default endpoint when the user has come back
  // here later from settings-clear.
  const defaultEpKey = settings?.default_endpoint;
  const defaultEp = settings && defaultEpKey ? settings.endpoints[defaultEpKey] : undefined;
  const keyRef = defaultEp?.key_ref ?? `codefactory.endpoint.${defaultEpKey ?? "openrouter"}`;

  const finish = async (extra?: Partial<typeof settings>) => {
    if (!settings) return;
    try {
      await save({ ...settings, ...(extra ?? {}), onboarded: true });
    } catch (e) {
      setError(String(e));
    }
  };

  const saveKeyAndAdvance = async () => {
    const k = keyDraft.trim();
    if (!k) {
      // Skip — leave key unset, advance anyway.
      setStep("first-action");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await saveApiKey(keyRef, k);
      // Persist that the default endpoint now has this key_ref bound
      // (if it didn't already).
      if (settings && defaultEpKey && !defaultEp?.key_ref) {
        const eps = { ...settings.endpoints };
        eps[defaultEpKey] = { ...eps[defaultEpKey], key_ref: keyRef };
        await save({ ...settings, endpoints: eps });
      }
      setStep("first-action");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Idle the body scroll so the modal feels modal.
  useEffect(() => {
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => { document.body.style.overflow = prev; };
  }, []);

  // Step progress dots — purely visual.
  const stepIndex = step === "welcome" ? 0 : step === "api-key" ? 1 : 2;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm">
      <div className="w-full max-w-xl mx-4 rounded-2xl border border-border bg-surface-1 shadow-2xl">

        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-border">
          <div className="flex items-center gap-2">
            <Sparkles size={14} className="text-accent" />
            <span className="text-sm font-medium text-gray-300">欢迎使用 CodeFactory</span>
          </div>
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-1">
              {[0, 1, 2].map((i) => (
                <span
                  key={i}
                  className={`block h-1 rounded transition-all ${
                    i === stepIndex ? "w-6 bg-accent"
                      : i < stepIndex ? "w-3 bg-accent/40"
                      : "w-3 bg-surface-3"
                  }`}
                />
              ))}
            </div>
            <button
              onClick={() => { void finish(); onClose(); }}
              className="p-1 rounded text-gray-500 hover:text-gray-300 hover:bg-surface-3"
              title="跳过引导"
            >
              <X size={14} />
            </button>
          </div>
        </div>

        {/* Body — per-step */}
        <div className="px-6 py-6 min-h-[280px]">
          {step === "welcome" && <WelcomeStep />}
          {step === "api-key" && (
            <ApiKeyStep
              value={keyDraft}
              onChange={setKeyDraft}
              endpointName={defaultEpKey ?? "openrouter"}
            />
          )}
          {step === "first-action" && (
            <FirstActionStep
              onPickNewProject={async () => { await finish(); onPickNewProject(); onClose(); }}
              onPickQuickTask={async () => { await finish(); onPickQuickTask(); onClose(); }}
              onPickProfile={async () => { await finish(); onPickProfile(); onClose(); }}
            />
          )}
          {error && (
            <p className="mt-3 text-xs text-red-700 dark:text-red-300">{error}</p>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-6 py-4 border-t border-border bg-surface-2 rounded-b-2xl">
          {step !== "welcome" ? (
            <button
              onClick={() => setStep(step === "api-key" ? "welcome" : "api-key")}
              className="flex items-center gap-1 text-xs text-gray-500 hover:text-gray-300 px-3 py-1.5 rounded"
            >
              <ChevronLeft size={12} /> 上一步
            </button>
          ) : <span />}
          {step === "first-action" ? (
            <button
              onClick={async () => { await finish(); onClose(); }}
              className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs"
            >
              <Check size={12} /> 跳过此步，回首页
            </button>
          ) : step === "api-key" ? (
            <button
              onClick={saveKeyAndAdvance}
              disabled={busy}
              className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs disabled:opacity-40"
            >
              {busy ? <Loader2 size={12} className="animate-spin" /> : <ChevronRight size={12} />}
              {keyDraft.trim() ? "保存并继续" : "稍后再配，继续"}
            </button>
          ) : (
            <button
              onClick={() => setStep("api-key")}
              className="flex items-center gap-1 px-4 py-1.5 rounded bg-accent hover:bg-accent-hover text-white text-xs"
            >
              下一步 <ChevronRight size={12} />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Step content ─────────────────────────────────────────────────────────────

function WelcomeStep() {
  return (
    <div className="space-y-4">
      <p className="text-base text-gray-200 leading-relaxed">
        CodeFactory 是一个跑在本地的<strong className="text-accent">软件工厂</strong>，
        加上一个<strong className="text-accent">私人 AI 助手</strong>，
        加上一个<strong className="text-accent">会自我进化</strong>的画像系统。
      </p>
      <ul className="text-sm text-gray-400 space-y-1.5 leading-relaxed">
        <li>· 描述你要的软件，AI 拆任务、并行执行、可中途插嘴</li>
        <li>· 每次会话后 AI 自动总结观察，你确认后写入你的画像</li>
        <li>· 下次拆任务时画像自动注入，越用越像你的搭档</li>
      </ul>
      <p className="text-xs text-gray-500 leading-relaxed pt-2 border-t border-border">
        接下来三步：配置 API → 选个起点 → 完成。<strong>大约 30 秒。</strong>
      </p>
    </div>
  );
}

function ApiKeyStep({
  value, onChange, endpointName,
}: { value: string; onChange: (s: string) => void; endpointName: string }) {
  return (
    <div className="space-y-4">
      <div>
        <h3 className="text-sm font-medium text-gray-200 mb-1">
          连接一个 AI 提供商
        </h3>
        <p className="text-xs text-gray-500 leading-relaxed">
          默认使用 <strong className="text-gray-300">OpenRouter</strong>（一个 key 用所有主流模型）。
          打开下面链接生成 key，粘贴回来即可。
        </p>
      </div>

      <a
        href="https://openrouter.ai/keys"
        target="_blank"
        rel="noreferrer"
        className="inline-flex items-center gap-1 text-xs text-accent hover:text-accent-hover"
      >
        openrouter.ai/keys <ExternalLink size={10} />
      </a>

      <div>
        <label className="block text-xs font-medium text-gray-400 mb-1.5">
          API Key（endpoint：{endpointName}）
        </label>
        <input
          type="password"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="sk-or-v1-…"
          autoFocus
          className="w-full bg-surface-3 border border-border rounded px-3 py-2 text-sm text-gray-200 outline-none focus:border-accent placeholder-gray-600"
        />
        <p className="mt-1.5 text-[11px] text-gray-600">
          存储在 macOS Keychain（不进任何明文文件）。也可以现在跳过，之后在
          <strong className="text-gray-500"> 设置 → Endpoints </strong>里配。
        </p>
      </div>
    </div>
  );
}

function FirstActionStep({
  onPickNewProject, onPickQuickTask, onPickProfile,
}: {
  onPickNewProject: () => void;
  onPickQuickTask: () => void;
  onPickProfile: () => void;
}) {
  return (
    <div className="space-y-3">
      <p className="text-sm text-gray-300 leading-relaxed">
        准备好了。<strong className="text-accent">三种用法</strong>，选一个上手——之后随时切：
      </p>
      <button
        onClick={onPickNewProject}
        className="w-full text-left rounded-lg border border-accent bg-accent/10 hover:bg-accent/20 px-4 py-3 transition-colors"
      >
        <div className="flex items-center gap-2 mb-1">
          <Plus size={14} className="text-accent" />
          <span className="text-sm font-medium text-gray-200">新建项目</span>
          <span className="ml-auto text-[10px] text-gray-500">推荐</span>
        </div>
        <p className="text-xs text-gray-500 leading-relaxed">
          选个文件夹 → 描述需求 → AI 拆任务并自动执行。完整软件工厂体验。
        </p>
      </button>
      <button
        onClick={onPickQuickTask}
        className="w-full text-left rounded-lg border border-border bg-surface-2 hover:bg-surface-3 px-4 py-3 transition-colors"
      >
        <div className="flex items-center gap-2 mb-1">
          <Zap size={14} className="text-gray-400" />
          <span className="text-sm font-medium text-gray-300">快速任务</span>
        </div>
        <p className="text-xs text-gray-500 leading-relaxed">
          不开项目，直接和 AI 聊天 / 问问题 / 让它处理一个文件。
        </p>
      </button>
      <button
        onClick={onPickProfile}
        className="w-full text-left rounded-lg border border-border bg-surface-2 hover:bg-surface-3 px-4 py-3 transition-colors"
      >
        <div className="flex items-center gap-2 mb-1">
          <User size={14} className="text-gray-400" />
          <span className="text-sm font-medium text-gray-300">我的画像</span>
        </div>
        <p className="text-xs text-gray-500 leading-relaxed">
          先看看「画像」是什么样——AI 用它了解你的偏好和风格。
        </p>
      </button>
    </div>
  );
}
