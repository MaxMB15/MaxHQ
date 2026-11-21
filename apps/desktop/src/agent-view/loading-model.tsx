// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The quiet step above a local agent's composer while its turn waits for
// the model to load into memory. It shows as soon as the turn starts, so a
// cold first turn never reads as a blank wait.

import type { Agent } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { localLoading } from "@maxhq/shared/local-models/running";

import { useView } from "@/app/services";

import { Spinner } from "./icons";

/** "Loading model" with the model's name, while a local agent's model loads. */
export const LoadingModel = ({ agent }: { agent: Agent }) => {
	useI18n();
	// The model's name, empty when the server doesn't list it, or null while nothing loads.
	const model = useView((s) => {
		const loading = localLoading(s.view, agent);
		return loading ? (loading.model ?? "") : null;
	});
	if (model === null) return null;
	return (
		<div className="shrink-0 px-7 pb-2">
			<div
				role="status"
				data-loading-model
				className="mx-auto flex max-w-[744px] min-w-0 items-center gap-2 px-1 text-[12px] text-muted-foreground"
			>
				<Spinner size={11} />
				<span className="shrink-0 font-medium text-secondary-foreground">
					{translate("Loading model")}
				</span>
				{model ? <span className="truncate">{model}</span> : null}
			</div>
		</div>
	);
};
