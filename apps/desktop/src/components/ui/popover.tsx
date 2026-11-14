// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as arrow
// functions. The panel variant, the touch backdrop, the glass background and
// the animated viewport are left out, since MaxHQ's popovers are plain cards
// on a desktop.

import { Popover as PopoverPrimitive } from "@base-ui/react/popover";

import { cn } from "@/lib/utils";

/** Holds a trigger and the popover it opens on click. */
export const Popover = PopoverPrimitive.Root;

/** The button that opens the popover. */
export const PopoverTrigger = ({
	className,
	children,
	...props
}: PopoverPrimitive.Trigger.Props) => (
	<PopoverPrimitive.Trigger className={className} data-slot="popover-trigger" {...props}>
		{children}
	</PopoverPrimitive.Trigger>
);

/** The popover itself, placed next to its trigger. */
export const PopoverPopup = ({
	className,
	side = "bottom",
	align = "center",
	sideOffset = 4,
	alignOffset = 0,
	anchor,
	...props
}: PopoverPrimitive.Popup.Props & {
	side?: PopoverPrimitive.Positioner.Props["side"];
	align?: PopoverPrimitive.Positioner.Props["align"];
	sideOffset?: PopoverPrimitive.Positioner.Props["sideOffset"];
	alignOffset?: PopoverPrimitive.Positioner.Props["alignOffset"];
	/** What to place it next to, when that isn't the trigger. */
	anchor?: PopoverPrimitive.Positioner.Props["anchor"];
}) => (
	<PopoverPrimitive.Portal>
		<PopoverPrimitive.Positioner
			align={align}
			alignOffset={alignOffset}
			anchor={anchor}
			className="z-[130] max-w-(--available-width)"
			data-slot="popover-positioner"
			side={side}
			sideOffset={sideOffset}
		>
			<PopoverPrimitive.Popup
				className={cn(
					"relative max-h-(--available-height) origin-(--transform-origin) overflow-y-auto rounded-lg border bg-popover text-popover-foreground outline-none transition-[scale,opacity] data-ending-style:scale-98 data-ending-style:opacity-0 data-starting-style:scale-98 data-starting-style:opacity-0",
					className
				)}
				data-slot="popover-popup"
				{...props}
			/>
		</PopoverPrimitive.Positioner>
	</PopoverPrimitive.Portal>
);

/** The popover's heading, which also names it for screen readers. */
export const PopoverTitle = ({ className, ...props }: PopoverPrimitive.Title.Props) => (
	<PopoverPrimitive.Title
		className={cn("text-sm leading-none font-semibold", className)}
		data-slot="popover-title"
		{...props}
	/>
);
