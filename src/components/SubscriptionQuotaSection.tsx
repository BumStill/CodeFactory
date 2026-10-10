// SPDX-License-Identifier: Apache-2.0
import { useCallback, useEffect, useState } from "react";
import { Gauge } from "lucide-react";
import { invoke } from "../lib/tauri";
import { useSettingsStore } from "../stores/settings";
import type { SubscriptionQuotaStatus } from "../lib/tauri";

/** The share of each window CodeFactory uses when the user has not chosen one. */
export const DEFAULT_QUOTA_CAP_PERCENT = 80;

type WindowKey = "five_hour_percent" | "weekly_percent";

/**
 * Render an epoch-ms reset instant as a local clock hint, or `null` when the
 * window has no known reset.
 */
export function resetClockHint(resetsAtMs?: number | null): string | null {
  if (!resetsAtMs) return null;
  const date = new Date(resetsAtMs);
  if (Number.isNaN(date.getTime()) || date.getTime() <= Date.now()) return null;
  const clock = date.toLocaleTimeString("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  return `约 ${clock} 恢复`;
}

/**
 * CF-QUOTA-R4: show how much of each subscription window CodeFactory has used,
 * at what cap, and when it will return — and let the user change the cap
 * without leaving the page.
 */
export function SubscriptionQuotaSection() {
  const { settings, save } = useSettingsStore();
  const [statuses, setStatuses] = useState<SubscriptionQuotaStatus[] | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    try {
      const result = await invoke<SubscriptionQuotaStatus[]>("subscription_quota_status");
      // Any environment that does not answer with a list (an unmocked command in
      // a test, or a build where the command is absent) simply has nothing to
      // show — never crash the Settings page over it.
      setStatuses(Array.isArray(result) ? result : []);
    } catch {
      setStatuses([]);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const setCap = async (endpoint: string, window: WindowKey, value: number) => {
    if (!settings || !Number.isFinite(value)) return;
    const clamped = Math.max(1, Math.min(100, Math.round(value)));
    const caps = { ...(settings.subscription_quota_caps ?? {}) };
    const current = caps[endpoint] ?? {
      five_hour_percent: DEFAULT_QUOTA_CAP_PERCENT,
      weekly_percent: DEFAULT_QUOTA_CAP_PERCENT,
    };
    caps[endpoint] = { ...current, [window]: clamped };
    setBusy(true);
    try {
      await save({ ...settings, subscription_quota_caps: caps });
      await reload();
    } finally {
      setBusy(false);
    }
  };

  if (statuses !== null && statuses.length === 0) return null;

  return (
    <section
      aria-labelledby="subscription-quota-title"
      className="space-y-3 rounded-xl border border-border bg-surface-1 p-3"
    >
      <div className="flex items-center gap-2">
        <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-accent/15 text-accent">
          <Gauge size={16} />
        </div>
        <div className="min-w-0">
          <p id="subscription-quota-title" className="text-body text-gray-200">
            订阅用量上限
          </p>
          <p className="text-label text-gray-500">
            到上限后 CodeFactory 会自动改用下一个端点，给你自己留余量；窗口重置后自动切回订阅端点。
          </p>
        </div>
      </div>

      {statuses === null ? (
        <p className="text-label text-gray-600">正在读取订阅用量…</p>
      ) : (
        <ul className="space-y-2">
          {statuses.map((status) => {
            const fiveHourHint = resetClockHint(status.five_hour_resets_at_ms);
            const weeklyHint = resetClockHint(status.weekly_resets_at_ms);
            return (
              <li
                key={status.endpoint}
                className="space-y-2 rounded-lg border border-border/70 bg-surface-2 p-2.5"
              >
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-body text-gray-200">{status.label}</span>
                  <span className="rounded-full border border-border px-1.5 py-0.5 text-caption text-gray-500">
                    {status.source}
                  </span>
                  {status.over_cap && (
                    <span
                      role="status"
                      className="rounded-full border border-amber-500/40 bg-amber-500/10 px-1.5 py-0.5 text-caption text-amber-700 dark:text-amber-400"
                    >
                      已达上限，已切到其它端点，为你保留余量
                    </span>
                  )}
                </div>

                <p className="text-label text-gray-400">
                  本窗口已用 {status.five_hour_percent}%（上限 {status.five_hour_cap_percent}%）
                  {fiveHourHint ? ` · ${fiveHourHint}` : ""}
                </p>
                <p className="text-label text-gray-400">
                  本周已用 {status.weekly_percent}%（上限 {status.weekly_cap_percent}%）
                  {weeklyHint ? ` · ${weeklyHint}` : ""}
                </p>

                <div className="flex flex-wrap items-center gap-2">
                  <label className="flex items-center gap-1.5 text-label text-gray-400">
                    窗口上限 %
                    <input
                      type="number"
                      min={1}
                      max={100}
                      disabled={busy || !settings}
                      aria-label={`${status.label} 五小时窗口上限百分比`}
                      className="w-16 rounded border border-border bg-surface-1 px-1.5 py-0.5 text-label text-gray-200"
                      value={status.five_hour_cap_percent}
                      onChange={(event) => void setCap(status.endpoint, "five_hour_percent", event.target.valueAsNumber)}
                    />
                  </label>
                  <label className="flex items-center gap-1.5 text-label text-gray-400">
                    每周上限 %
                    <input
                      type="number"
                      min={1}
                      max={100}
                      disabled={busy || !settings}
                      aria-label={`${status.label} 每周上限百分比`}
                      className="w-16 rounded border border-border bg-surface-1 px-1.5 py-0.5 text-label text-gray-200"
                      value={status.weekly_cap_percent}
                      onChange={(event) => void setCap(status.endpoint, "weekly_percent", event.target.valueAsNumber)}
                    />
                  </label>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
