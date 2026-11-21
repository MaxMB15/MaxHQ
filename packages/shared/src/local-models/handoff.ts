// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How each end of a move names itself in the timeline. The move says which
// end ran on a local model (`from_local` and `to_local`, read through
// `handoffKinds`), so an end shows as "Local · <model>" from the item alone;
// the view only gives the model its name.

import { handoffEnd } from "../agent-view/format";
import type { AgentKind } from "../status/agent-kind";
import { modelName } from "../view/select";
import type { ViewState } from "../view/replica";
import { serverModelName } from "./models";

/** A local model's name, from the first server that lists it, else its ID. */
const localModelLabel = (view: ViewState, model: string): string => {
	for (const server of view.tables.model_server.values()) {
		const found = server.models.find((m) => m.id === model);
		if (found) return serverModelName(view, server, found);
	}
	return model;
};

/** One end of a move, such as "Codex gpt-5" or "Local · Qwen3.6-35B-A3B". */
export const handoffEndText = (view: ViewState, kind: AgentKind, model: string | null): string => {
	if (model === null) return handoffEnd(kind, null);
	return handoffEnd(
		kind,
		kind === "local" ? localModelLabel(view, model) : modelName(view, kind, model)
	);
};
