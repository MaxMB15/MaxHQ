// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How each end of a move names itself in the timeline. A move records only
// vendors, and a local agent's vendor is Claude on the wire, so an end
// whose model a local account's server lists shows as "Local · <model>".

import type { Vendor } from "../bindings";
import { handoffEnd } from "../agent-view/format";
import { isLocalAccount } from "../status/agent-kind";
import { modelName } from "../view/select";
import type { ViewState } from "../view/replica";
import { serverModelName, serverOf } from "./models";
import { localAgentLabel } from "./wording";

/** One end of a move, such as "Codex gpt-5" or "Local · Qwen3.6 35B A3B". */
export const handoffEndText = (view: ViewState, vendor: Vendor, model: string | null): string => {
	if (vendor === "claude" && model !== null) {
		for (const account of view.tables.account.values()) {
			if (!isLocalAccount(account)) continue;
			const server = serverOf(view, account);
			const found = server?.models.find((m) => m.id === model);
			if (server && found) return localAgentLabel(serverModelName(view, server, found));
		}
	}
	return handoffEnd(vendor, model ? modelName(view, vendor, model) : null);
};
