// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import {
  CHATGPT_FALLBACK_MODELS,
  selectChatGptCatalog,
  selectChatGptDefaultModel,
} from "./chatgptModels";
import type { CustomModel } from "./tauri";

describe("ChatGPT model capability catalog", () => {
  it("falls back to the current visible Codex subscription models", () => {
    expect(CHATGPT_FALLBACK_MODELS[0]).toMatchObject({
      id: "gpt-5.6-sol",
      context_length: 272000,
      default_reasoning_effort: "low",
      supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    });
    expect(CHATGPT_FALLBACK_MODELS.find((model) => model.id === "gpt-5.5"))
      .toMatchObject({ supported_reasoning_efforts: ["low", "medium", "high", "xhigh"] });
  });

  it("prefers a non-empty live catalog and otherwise uses the bundled snapshot", () => {
    const live: CustomModel[] = [{ id: "future-model", supported_reasoning_efforts: ["high"] }];
    expect(selectChatGptCatalog(live)).toEqual(live);
    expect(selectChatGptCatalog([])).toEqual(CHATGPT_FALLBACK_MODELS);
    expect(selectChatGptCatalog(null)).toEqual(CHATGPT_FALLBACK_MODELS);
  });

  it("uses the first server model when the bundled default is no longer visible", () => {
    expect(selectChatGptDefaultModel([{ id: "future-model" }])).toBe("future-model");
    expect(selectChatGptDefaultModel(CHATGPT_FALLBACK_MODELS)).toBe("gpt-5.6-sol");
  });
});
