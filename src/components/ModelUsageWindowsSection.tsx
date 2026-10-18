// SPDX-License-Identifier: Apache-2.0
import { useEffect, useState } from "react";
import { useSettingsStore } from "../stores/settings";
import { applyPreset, evaluateWindow, modelWindowKey, type UsageWindow } from "../lib/usageWindows";
import { setUsageWindow } from "../lib/modelUsageWindows";
/** Years the bundled notice covers, so the page can state the fallback honestly. */
export function coveredYears(settings: { endpoint_usage_windows?: Record<string, { calendar?: { years?: number[] } }>; model_usage_windows?: Record<string, { calendar?: { years?: number[] } }> }): number[] {
  const years = new Set<number>();
  for (const rule of [...Object.values(settings.endpoint_usage_windows ?? {}), ...Object.values(settings.model_usage_windows ?? {})]) {
    for (const year of rule.calendar?.years ?? []) years.add(year);
  }
  return [...years].sort();
}
export function ModelUsageWindowsSection() {
  const { settings, save } = useSettingsStore();
  const [now, setNow] = useState(Date.now());
  const [error, setError] = useState("");
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 30000); return () => clearInterval(timer); }, []);
  if (!settings) return null;
  const update = async (endpoint: string, model: string | undefined, rule: UsageWindow | null) => {
    try { await setUsageWindow({ endpoint, model, rule }, { settings, save }); setError(""); } catch (e) { setError(String(e)); }
  };
  return <section aria-labelledby="model-windows-title" className="space-y-3 rounded-xl border border-border bg-surface-1 p-3">
    <h2 id="model-windows-title" className="text-body text-gray-200">模型使用时段</h2>
    <p className="text-label text-gray-500">先设置服务商默认时段，新模型自动继承；模型可自定义例外。旧规则不一致时保留模型例外，不自动合并。缺少年份节假日数据时按工作日高峰处理。</p>
    <p className="text-label text-gray-600">已内置节假日／调休数据的年份：{coveredYears(settings).join("、") || "无"}。其余年份按保守口径（工作日算高峰）。</p>
    {error && <p role="alert" className="text-label text-red-500">{error}</p>}
    {Object.entries(settings.endpoints).flatMap(([endpoint, config]) => {
      const models = [...new Set([...(config.custom_models ?? []).map(m => m.id), ...(config.active_model ? [config.active_model] : [])])];
      return [undefined, ...models].map(model => {
        const own = model === undefined ? undefined : settings.model_usage_windows?.[modelWindowKey(endpoint, model)];
        const rule = own ?? settings.endpoint_usage_windows?.[endpoint] ?? { mode: "anytime" as const };
        const status = evaluateWindow(rule, now);
        return <div key={model === undefined ? endpoint : modelWindowKey(endpoint, model)} className="min-w-0 space-y-2 rounded-lg border border-border bg-surface-2 p-3">
          <p className="break-all text-body text-gray-200">{endpoint} / {model ?? "服务商默认"}{model !== undefined && <span> · {own ? "自定义" : "继承服务商"}</span>}</p>
          {model !== undefined && <button className="text-label text-accent" onClick={() => void update(endpoint, model, own ? null : rule)}>{own ? "恢复继承服务商" : "切换自定义"}</button>}
          <p role="status" className="text-label text-gray-400">{status.available ? "现在可用" : "当前不在允许时段"}{status.next_transition_ms ? ` · 下一次切换：${new Date(status.next_transition_ms).toLocaleString("zh-CN", { timeZone: rule.timezone ?? "UTC" })}（${rule.timezone ?? "UTC"}）` : " · 无切换时间"}</p>
          <select aria-label={`${endpoint} ${model ?? "服务商默认"} 使用时段`} value={rule.mode} onChange={e => void update(endpoint, model, e.target.value === "anytime" ? {mode: "anytime"} : e.target.value === "official_off_peak" ? applyPreset("deepseek_off_peak") : { ...applyPreset("deepseek_off_peak"), mode: "custom", invert: false })} className="max-w-full rounded border border-border bg-surface-1 p-1 text-body text-gray-200">
            <option value="anytime">任何时间</option><option value="official_off_peak">仅官方闲时（DeepSeek 预设）</option><option value="custom">自定义时段</option>
          </select>
          {rule.mode !== "anytime" && <div className="flex min-w-0 flex-wrap gap-2">
            <label className="text-label text-gray-400">时区 <input aria-label={`${model} 时区`} className="max-w-full rounded border border-border bg-surface-1 p-1" value={rule.timezone ?? "UTC"} onChange={e => void update(endpoint, model, {...rule, timezone: e.target.value})}/></label>
            <label className="text-label text-gray-400">星期（0=日） <input aria-label={`${model} 星期`} className="w-32 rounded border border-border bg-surface-1 p-1" value={(rule.weekdays ?? []).join(",")} onChange={e => void update(endpoint, model, {...rule, weekdays: e.target.value.split(",").map(Number)})}/></label>
            <label className="text-label text-gray-400"><input type="checkbox" checked={rule.exclude_holidays ?? false} onChange={e => void update(endpoint, model, {...rule, exclude_holidays: e.target.checked})}/>排除中国节假日</label>
            <label className="text-label text-gray-400"><input type="checkbox" checked={rule.invert ?? false} onChange={e => void update(endpoint, model, {...rule, invert: e.target.checked})}/>仅在所列时段之外使用</label>
            {(rule.periods ?? []).map((period, i) => <div className="flex flex-wrap gap-1" key={i}>{(["start", "end"] as const).map(field => <input key={field} aria-label={`${model} 时段 ${i+1} ${field}`} type="time" value={period[field]} className="rounded border border-border bg-surface-1 p-1 text-label text-gray-200" onChange={e => void update(endpoint, model, {...rule, periods: rule.periods?.map((p,j) => j === i ? {...p,[field]: e.target.value} : p)})}/>)}<button onClick={() => void update(endpoint, model, {...rule, periods: rule.periods?.filter((_,j) => i !== j)})} className="text-label text-gray-400">删除时段</button></div>)}
            <button onClick={() => void update(endpoint, model, {...rule, periods: [...rule.periods ?? [], {start:"09:00",end:"18:00"}]})} className="text-label text-accent">添加时段</button>
          </div>}
        </div>;
      });
    })}
  </section>;
}
