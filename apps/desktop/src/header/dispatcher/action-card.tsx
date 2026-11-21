// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The card for a change the dispatcher made without asking, such as
// "Renamed Fix login to Auth fix", or an agent or a task it started, with
// its Undo. Undo puts the change back once, or stops the agent or deletes
// the task. It greys out after `maxd` restarts or once something else
// changed the same thing, and `maxd` checks again when you press it.

import { Check } from "lucide-react";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { SecondaryButton } from "@/agent-view/items/card";
import type { ItemOf } from "@/agent-view/items/types";
import { failureOf } from "@maxhq/shared/data/failure";
import { actionCard } from "@maxhq/shared/dispatch/action";
import { stepCard } from "@maxhq/shared/dispatch/plan";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** A change the dispatcher made at once, with Undo while it can still be undone. */
export const DispatcherActionCard = ({
	item,
}: {
	item: ItemOf<"dispatcher_action"> | ItemOf<"dispatcher_step">;
}) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const card = item.kind === "dispatcher_action" ? actionCard(view, item) : stepCard(view, item);
	const [undo, setUndo] = useState<{ step: "idle" | "busy" | "done"; error: string | null }>({
		step: "idle",
		error: null,
	});
	const press = async () => {
		if (!card.home || item.undo === null) return;
		setUndo({ step: "busy", error: null });
		try {
			const failure = failureOf(await source.undoDispatcherAction(card.home, item.undo));
			setUndo({ step: failure ? "idle" : "done", error: failure });
		} catch (error) {
			console.warn(translate("Couldn't undo the dispatcher's change"), error);
			setUndo({ step: "idle", error: translate("Couldn't undo it. Try again.") });
		}
	};
	return (
		<div className="flex items-start gap-2.5 rounded-lg border border-border px-3 py-2 text-[12.5px]">
			<Check
				size={14}
				strokeWidth={1.9}
				aria-hidden
				className="mt-[3px] shrink-0 text-muted-foreground"
			/>
			<div className="flex min-w-0 flex-1 flex-col gap-0.5">
				<p
					data-find="plain"
					className="leading-normal break-words text-foreground select-text"
				>
					{card.line}
				</p>
				{card.elsewhere ? (
					<p className="text-[11.5px] text-muted-foreground">
						{translate("On {{device}}", { device: card.elsewhere })}
					</p>
				) : null}
				{undo.error ? (
					<p role="alert" className="text-[11.5px] text-destructive">
						{undo.error}
					</p>
				) : null}
			</div>
			{undo.step === "done" ? (
				<span className="shrink-0 pt-1 text-[11.5px] text-muted-foreground">
					{translate("Undone")}
				</span>
			) : card.undo ? (
				<span
					className="shrink-0"
					title={
						card.undo === "gone"
							? translate("This change can't be undone any more")
							: undefined
					}
				>
					<SecondaryButton
						onClick={() => void press()}
						disabled={card.undo === "gone" || undo.step === "busy"}
					>
						{translate("Undo")}
					</SecondaryButton>
				</span>
			) : null}
		</div>
	);
};
