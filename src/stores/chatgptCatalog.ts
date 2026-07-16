// SPDX-License-Identifier: Apache-2.0
import { codexAccount, codexModels } from "../lib/tauri";
import {
  CHATGPT_BASE_URL,
  CHATGPT_ENDPOINT_KEY,
  selectChatGptCatalog,
  selectChatGptDefaultModel,
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

  // Read settings after the network request so a concurrent user save is not
  // overwritten by the snapshot that existed when the refresh started.
  const { settings, save } = useSettingsStore.getState();
  if (!settings) return;

  const existing = settings.endpoints[CHATGPT_ENDPOINT_KEY];
  const hasLastKnownCapabilities = existing?.custom_models?.some(
    (model) => model.supported_reasoning_efforts?.length || model.default_reasoning_effort,
  );
  if (!liveModels && hasLastKnownCapabilities) return;

  const models = selectChatGptCatalog(liveModels);
  if (existing && JSON.stringify(existing.custom_models ?? []) === JSON.stringify(models)) return;

  const validIds = models.map((model) => model.id);
  const active =
    existing?.active_model && validIds.includes(existing.active_model)
      ? existing.active_model
      : selectChatGptDefaultModel(models);

  await save({
    ...settings,
    endpoints: {
      ...settings.endpoints,
      [CHATGPT_ENDPOINT_KEY]: {
        base_url: CHATGPT_BASE_URL,
        api_style: "chatgpt",
        custom_models: models,
        active_model: active,
      },
    },
    default_endpoint: existing ? settings.default_endpoint : CHATGPT_ENDPOINT_KEY,
    default_model:
      existing &&
      (settings.default_endpoint !== CHATGPT_ENDPOINT_KEY || validIds.includes(settings.default_model))
        ? settings.default_model
        : active,
  });
}
