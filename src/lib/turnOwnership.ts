// SPDX-License-Identifier: Apache-2.0

import type { UIMessage } from "../stores/chatEvents";

type ObjectiveStatus = NonNullable<
  NonNullable<UIMessage["turnActivity"]>["objectiveStatus"]
>;

/// Objective states in which the system has released the turn: nothing is
/// running and whatever happens next needs the user. The composer's stop
/// button and the progress banner both depend on this, and they used to keep
/// private copies of the list — the banner's copy was missing
/// `waiting_core_input`, so a turn that had already handed control back kept
/// advertising a next step, a live timer and a remaining-time estimate.
export const SYSTEM_RELEASED_OBJECTIVE_STATUSES: readonly ObjectiveStatus[] = [
  "waiting_core_input",
  "completed",
  "cancelled",
  "legacy_orphan",
];

/// True while the objective still belongs to the system, i.e. the user cannot
/// end it and the UI may legitimately present it as in flight.
export function systemOwnsObjective(status: string | null | undefined): boolean {
  return Boolean(
    status && !SYSTEM_RELEASED_OBJECTIVE_STATUSES.includes(status as ObjectiveStatus),
  );
}

export function rootTurnIdForMessage(message: UIMessage): string | null {
  return (
    message.rootTurnId ??
    message.turnActivity?.rootTurnId ??
    message.plan?.rootTurnId ??
    null
  );
}

export interface CurrentTurnOwnership {
  rootTurnId: string | null;
  messages: UIMessage[];
  released: boolean;
  systemHeld: boolean;
}

/**
 * Resolve ownership for the latest durable root turn. Mid-run steers are user
 * rows too, but they do not start a new root; the assistant bubbles on either
 * side retain the original root identity. Consumers must therefore group by
 * root identity instead of treating the last role=user row as a turn boundary.
 */
export function currentTurnOwnership(messages: UIMessage[]): CurrentTurnOwnership {
  const knownRootIds = new Set(
    messages.map(rootTurnIdForMessage).filter((id): id is string => Boolean(id)),
  );
  let latestKnownRootId: string | null = null;
  let latestKnownRootIndex = -1;
  let latestUserIndex = -1;
  for (let index = 0; index < messages.length; index += 1) {
    if (messages[index].role === "user") latestUserIndex = index;
    const candidate = rootTurnIdForMessage(messages[index]);
    if (candidate) {
      latestKnownRootId = candidate;
      latestKnownRootIndex = index;
    }
  }
  // A user row after every durable projection is a newly submitted root. If
  // a later assistant binds back to the previous root, this same row is a
  // steer and latestKnownRootIndex moves past it.
  const rootTurnId =
    latestUserIndex > latestKnownRootIndex
      ? messages[latestUserIndex]?.id ?? null
      : latestKnownRootId ?? messages[latestUserIndex]?.id ?? null;
  const rootIndex = rootTurnId
    ? messages.findIndex(
        (message) => message.role === "user" && message.id === rootTurnId,
      )
    : -1;
  const fallbackUserIndex = messages.reduce(
    (latest, message, index) => (message.role === "user" ? index : latest),
    -1,
  );
  const startIndex = rootIndex >= 0 ? rootIndex : Math.max(0, fallbackUserIndex);
  let endIndex = messages.length;
  for (let index = startIndex + 1; index < messages.length && rootTurnId; index += 1) {
    const message = messages[index];
    if (
      message.role === "user" &&
      message.id !== rootTurnId &&
      knownRootIds.has(message.id)
    ) {
      endIndex = index;
      break;
    }
  }
  const turnMessages = messages.slice(startIndex, endIndex);
  const currentActivity = [...turnMessages]
    .reverse()
    .find((message) => message.turnActivity?.objectiveStatus)
    ?.turnActivity;
  const hasSettlement = turnMessages.some(
    (message) =>
      message.turnSettledAt != null || message.turnActivity?.terminalReason != null,
  );
  const objectiveStatus = currentActivity?.objectiveStatus;
  const released = Boolean(
    hasSettlement ||
    (objectiveStatus && !systemOwnsObjective(objectiveStatus)),
  );
  const systemHeld =
    !released &&
    turnMessages.some(
      (message) =>
        !message.turnActivity?.terminalReason &&
        systemOwnsObjective(message.turnActivity?.objectiveStatus),
    );
  return { rootTurnId, messages: turnMessages, released, systemHeld };
}
