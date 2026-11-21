// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The quiet step above a local agent's composer while its turn waits for
// the model to load into memory. It shows as soon as the turn starts, so a
// cold first turn never reads as a blank wait. Once the server says how far
// the load is, a bar shows it; at 99 the file is in memory and the step says
// "Warming up" until the server answers, so it never reads as stuck.

import { useShallow } from "zustand/react/shallow";

import type { Agent } from "@maxhq/shared/bindings";
import { formatNumber, t as translate, useI18n } from "@maxhq/shared/i18n";
import { localLoading } from "@maxhq/shared/local-models/running";
import { loadingLabel, warmingUp } from "@maxhq/shared/local-models/wording";

import { useView } from "@/app/services";
import { cn } from "@/lib/utils";

import { Spinner } from "./icons";

/** "Loading model" with the model's name and how far it is, while a local agent's model loads. */
export const LoadingModel = ({ agent }: { agent: Agent }) => {
	useI18n();
	// The model's name, empty when the server doesn't list it, and the percent; null while nothing loads.
	const loading = useView(
		useShallow((s) => {
			const loading = localLoading(s.view, agent);
			return loading ? { model: loading.model ?? "", percent: loading.percent } : null;
		})
	);
	if (loading === null) return null;
	const { model, percent } = loading;
	const warming = warmingUp(percent);
	return (
		<div className="shrink-0 px-7 pb-2">
			<div
				role="status"
				data-loading-model
				className="mx-auto flex max-w-[744px] min-w-0 items-center gap-2 px-1 text-[12px] text-muted-foreground"
			>
				<Spinner size={11} />
				<span className="shrink-0 font-medium text-secondary-foreground">
					{loadingLabel(percent)}
				</span>
				{model ? <span className="min-w-0 truncate">{model}</span> : null}
				{percent !== null ? (
					<span className="ml-auto flex shrink-0 items-center gap-2">
						<span
							role="progressbar"
							aria-label={translate("Load of {{model}}", { model })}
							aria-valuemin={0}
							aria-valuemax={100}
							aria-valuenow={Math.floor(percent)}
							className="h-1.5 w-24 overflow-hidden rounded-full bg-track"
						>
							<span
								className={cn(
									"block h-full rounded-full bg-primary transition-[width] duration-500",
									warming && "animate-pulse"
								)}
								style={{ width: `${percent}%` }}
							/>
						</span>
						{warming ? null : (
							<span className="w-8 text-right tabular-nums">
								{translate("{{percent}}%", {
									percent: formatNumber(Math.floor(percent)),
								})}
							</span>
						)}
					</span>
				) : null}
			</div>
		</div>
	);
};
