// SPDX-License-Identifier: Apache-2.0
import { applyCodexModels, codexAccount, codexModels } from "../lib/tauri";
import {
  CHATGPT_ENDPOINT_KEY,
  selectChatGptCatalog,
} from "../lib/chatgptModels";
import { useSettingsStore } from "./settings";

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
