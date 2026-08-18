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
