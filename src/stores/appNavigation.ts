// SPDX-License-Identifier: Apache-2.0
import { create } from "zustand";

export interface SkillReviewNavigation {
  skillId: string;
  originSessionId: string | null;
  originToolCallId: string;
}

interface ReceiptFocusTarget {
  toolCallId: string;
  skillId: string | null;
}

interface AppNavigationState {
  skillReview: SkillReviewNavigation | null;
  returnFocusToolCallId: string | null;
  returnFocusSkillId: string | null;
  requestSkillReview: (review: SkillReviewNavigation) => void;
  finishSkillReview: () => SkillReviewNavigation | null;
  restoreReceiptFocus: (toolCallId: string, skillId?: string | null) => void;
  consumeReceiptFocus: (target: ReceiptFocusTarget) => void;
  clearSkillReview: () => void;
  reset: () => void;
}

export const useAppNavigationStore = create<AppNavigationState>((set, get) => ({
  skillReview: null,
  returnFocusToolCallId: null,
  returnFocusSkillId: null,
  requestSkillReview: (skillReview) => set({
    skillReview,
    returnFocusToolCallId: null,
    returnFocusSkillId: null,
  }),
  finishSkillReview: () => {
    const review = get().skillReview;
    set({
      skillReview: null,
      returnFocusToolCallId: review?.originToolCallId ?? null,
      returnFocusSkillId: review?.skillId ?? null,
    });
    return review;
  },
  restoreReceiptFocus: (returnFocusToolCallId, returnFocusSkillId = null) => set({
    returnFocusToolCallId,
    returnFocusSkillId,
  }),
  consumeReceiptFocus: (target) => {
    const state = get();
    if (
      state.returnFocusToolCallId === target.toolCallId
      && (state.returnFocusSkillId == null || state.returnFocusSkillId === target.skillId)
    ) {
      set({ returnFocusToolCallId: null, returnFocusSkillId: null });
    }
  },
  clearSkillReview: () => set({ skillReview: null }),
  reset: () => set({
    skillReview: null,
    returnFocusToolCallId: null,
    returnFocusSkillId: null,
  }),
}));
