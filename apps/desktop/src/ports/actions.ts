// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What a port's buttons do on this Mac: forward it here, stop the forward,
// or open it in the browser, through the shared actions.

import { useServices } from "@/app/services";
import { type PortActions, usePortActions as useActions } from "@maxhq/shared/ports/actions";

export type { PortActions };

/** Actions on ports, each with its own progress and failure. */
export const usePortActions = (): PortActions => useActions(useServices().source);
