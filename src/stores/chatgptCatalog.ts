// SPDX-License-Identifier: Apache-2.0
import { applyCodexModels, codexAccount, codexModels } from "../lib/tauri";
import {
  CHATGPT_ENDPOINT_KEY,
  selectChatGptCatalog,
} from "../lib/chatgptModels";
import { useSettingsStore } from "./settings";

/** How long a completed sync stays authoritative. Opening the model picker is
 * a frequent, deliberate act; without a window, every open would hit the
 * network. Five minutes is short enough that a model published while the app
 * is running shows up the next time the user goes looking for it. */
const CATALOG_TTL_MS = 5 * 60 * 1000;
let lastSyncedAt = 0;
let inFlight: Promise<void> | null = null;

/** Refresh the catalog when it is worth refreshing, and never more than once
 * at a time.
 *
 * The catalog used to be fetched only at startup and on the Settings page, so
 * an app left open never learned about a newly published model. Re-picking the
 * endpoint in the composer appeared to fix it, but that only re-read what
 * `syncChatGptCatalog` had already written at launch — the server was not
 * consulted again.
 */
export async function refreshChatGptCatalogIfStale(): Promise<void> {
  if (Date.now() - lastSyncedAt < CATALOG_TTL_MS) return;
  if (inFlight) return inFlight;
  // Opening the model picker must never depend on this succeeding. The
  // transport can reject, and it can also throw synchronously when the
  // command is unavailable, which a promise `.catch` would not see — so the
  // call itself is wrapped, not just its result. A failed refresh leaves the
  // last known catalog in place and lets the next open try again.
  inFlight = (async () => {
    try {
      await syncChatGptCatalog();
      lastSyncedAt = Date.now();
    } catch {
      /* keep the last known catalog; retry on the next open */
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

/** Test seam: forget the last successful sync. */
export function resetChatGptCatalogFreshness(): void {
  lastSyncedAt = 0;
  inFlight = null;
}

/** Refresh the signed-in subscription endpoint from the official Codex model
 * catalog. The bundled snapshot keeps startup usable when refresh is offline. */
export async function syncChatGptCatalog(knownSignedIn = false): Promise<void> {
  if (!knownSignedIn) {
    const account = await codexAccount().catch(() => null);
    if (!account) return;
  }

  const fetched = await codexModels().catch(() => null);
  const liveModels = fetched?.length ? fetched : null;

  const { settings, load } = useSettingsStore.getState();
  if (!settings) return;

  const existing = settings.endpoints[CHATGPT_ENDPOINT_KEY];
  const hasLastKnownCapabilities = existing?.custom_models?.some(
    (model) => model.supported_reasoning_efforts?.length || model.default_reasoning_effort,
  );
  if (!liveModels && hasLastKnownCapabilities) return;

  const models = selectChatGptCatalog(liveModels);
  const validModelIds = new Set(models.map((model) => model.id));
  const selectionsAreValid =
    !!existing?.active_model &&
    validModelIds.has(existing.active_model) &&
    (settings.default_endpoint !== CHATGPT_ENDPOINT_KEY || validModelIds.has(settings.default_model));
  if (
    existing &&
    selectionsAreValid &&
    JSON.stringify(existing.custom_models ?? []) === JSON.stringify(models)
  ) {
    return;
  }

  // The backend applies an endpoint-scoped patch under the same settings lock
  // used by logout, so this refresh cannot overwrite unrelated concurrent saves
  // or resurrect the endpoint after the user signs out.
  await applyCodexModels(models);
  await load();
}
