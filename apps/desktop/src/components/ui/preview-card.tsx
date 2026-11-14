// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as arrow
// functions with the trigger's delays as props.

import { PreviewCard as PreviewCardPrimitive } from "@base-ui/react/preview-card";

import { cn } from "@/lib/utils";

/** Holds a trigger and the card it opens on hover or focus. */
export const PreviewCard = PreviewCardPrimitive.Root;

/** The element that opens the card. */
export const PreviewCardTrigger = (props: PreviewCardPrimitive.Trigger.Props) => (
	<PreviewCardPrimitive.Trigger data-slot="preview-card-trigger" {...props} />
);

/** The card, placed next to its trigger. */
export const PreviewCardPopup = ({
	className,
	align = "start",
	side = "top",
	sideOffset = 6,
	...props
}: PreviewCardPrimitive.Popup.Props & {
	align?: PreviewCardPrimitive.Positioner.Props["align"];
	side?: PreviewCardPrimitive.Positioner.Props["side"];
	sideOffset?: PreviewCardPrimitive.Positioner.Props["sideOffset"];
}) => (
	<PreviewCardPrimitive.Portal>
		<PreviewCardPrimitive.Positioner
			align={align}
			className="z-[140] max-w-(--available-width)"
			data-slot="preview-card-positioner"
			side={side}
			sideOffset={sideOffset}
		>
			<PreviewCardPrimitive.Popup
				className={cn(
					"origin-(--transform-origin) rounded-lg border bg-popover text-popover-foreground shadow-lg outline-none transition-[scale,opacity] data-ending-style:scale-98 data-ending-style:opacity-0 data-starting-style:scale-98 data-starting-style:opacity-0",
					className
				)}
				data-slot="preview-card-popup"
				{...props}
			/>
		</PreviewCardPrimitive.Positioner>
	</PreviewCardPrimitive.Portal>
);
