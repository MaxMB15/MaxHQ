// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The footer's effort picker, beside the model picker, for a Claude Code or
// Codex agent whose model lists effort levels. Its button names the level
// the agent runs at and opens a slider over the levels, with the one the
// model runs at when told none marked. A change applies from the agent's
// next message. In a narrow footer it folds to its icon.

import { ChevronDown, Gauge } from "lucide-react";
import { useRef, useState } from "react";

import { effortChoices, effortLevel, effortShown } from "@maxhq/shared/agent-view/effort";
import type { ModelChoice } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { StepSlider } from "@/components/step-slider";
import { Popover, PopoverPopup, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { FOLDED, GROUP_LABEL, MENU_POPUP, TRIGGER } from "./pickers";

/** Picks the effort level from what the agent's model lists. */
export const EffortPicker = ({
	effort,
	model,
	choices,
	folded = false,
	onPick,
}: {
	effort: string | null;
	model: string | null;
	choices: readonly ModelChoice[];
	folded?: boolean;
	onPick: (effort: string) => void;
}) => {
	useI18n();
	const [open, setOpen] = useState(false);
	const thumb = useRef<HTMLDivElement>(null);
	const levels = effortChoices(effort, model, choices);
	const level = effortLevel(effort, model, choices);
	if (levels.length === 0 || level === null) return null;
	const shown = effortShown(effort, model, choices);
	const label = translate("Effort: {{level}}", { level: shown });
	return (
		<Popover open={open} onOpenChange={setOpen}>
			<Tooltip content={folded ? label : null}>
				<PopoverTrigger className={cn(TRIGGER, folded && FOLDED.icon)} aria-label={label}>
					<Gauge aria-hidden className="size-[13px] shrink-0" strokeWidth={2} />
					{!folded && shown}
					{!folded && (
						<ChevronDown
							aria-hidden
							className="size-[9px] shrink-0"
							strokeWidth={2.6}
						/>
					)}
				</PopoverTrigger>
			</Tooltip>
			<PopoverPopup
				side="top"
				align="start"
				sideOffset={6}
				initialFocus={thumb}
				className={`${MENU_POPUP} w-auto`}
			>
				<PopoverTitle className={`${GROUP_LABEL} block leading-normal font-normal`}>
					{translate("Effort")}
				</PopoverTitle>
				<StepSlider
					label={translate("Effort")}
					stops={levels}
					value={level}
					onPick={onPick}
					onDone={() => setOpen(false)}
					thumbRef={thumb}
					className="mx-2 mt-1 mb-1.5"
				/>
			</PopoverPopup>
		</Popover>
	);
};
