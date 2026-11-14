// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A pill as it shows: its icon and words, with its preview on hover. The
// preview also opens while the pill is selected, and leaves focus where it
// was, so typing goes on in the text box.

import type { useRender } from "@base-ui/react/use-render";
import { type ReactNode, useState } from "react";

import { Popover, PopoverPopup, PopoverTrigger } from "@/components/ui/popover";

import { Pill, PillLabel } from "./pill";
import type { PillPresentation } from "./registry";

/** How long the pointer rests on a pill before its preview opens. */
export const PREVIEW_DELAY_MS = 400;

type ShownPillProps = Omit<useRender.ComponentProps<"span">, "children"> & {
	readonly presentation: PillPresentation;
	/** Holds the preview open, such as while the pill is selected. */
	readonly selected?: boolean;
	/** More for the preview, such as Retry and Remove for a file that failed. */
	readonly footer?: ReactNode;
};

/** A pill with its preview. */
export const ShownPill = ({ presentation, selected = false, footer, ...props }: ShownPillProps) => {
	const [hovered, setHovered] = useState(false);
	const { family, state, progress, icon, label, aria, preview } = presentation;
	const pill = (
		<Pill family={family} state={state} progress={progress} aria-label={aria} {...props} />
	);
	if (preview === null)
		return (
			<Pill family={family} state={state} progress={progress} aria-label={aria} {...props}>
				{icon}
				<PillLabel>{label}</PillLabel>
			</Pill>
		);
	return (
		<Popover
			open={selected || hovered}
			onOpenChange={(open, details) => {
				// A click on the pill selects it, so it doesn't toggle the preview.
				if (details.reason === "trigger-press") return;
				setHovered(open);
			}}
		>
			<PopoverTrigger
				openOnHover
				delay={PREVIEW_DELAY_MS}
				closeDelay={120}
				nativeButton={false}
				tabIndex={-1}
				render={pill}
			>
				{icon}
				<PillLabel>{label}</PillLabel>
			</PopoverTrigger>
			<PopoverPopup
				side="top"
				align="start"
				initialFocus={false}
				finalFocus={false}
				className="p-2.5 text-xs shadow-lg"
			>
				{preview}
				{footer}
			</PopoverPopup>
		</Popover>
	);
};
