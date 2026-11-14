// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A short label that opens over a control on hover or keyboard focus, for
// a control whose words folded away or don't say everything. Controls in
// one `TooltipGroup` open theirs at once after the first, so moving along
// a row doesn't wait at each one.

import { Tooltip as TooltipPrimitive } from "@base-ui/react/tooltip";
import type { ReactElement, ReactNode } from "react";

/** Lets the tooltips inside open at once while one of them is open. */
export const TooltipGroup = TooltipPrimitive.Provider;

/**
 * Shows `content` next to `children`, a control that takes a ref and its
 * props, such as a button or a menu's trigger. Without `content` it shows
 * nothing, and the control stays where it was, keeping its focus.
 */
export const Tooltip = ({
	content,
	side = "top",
	children,
}: {
	content: ReactNode;
	side?: TooltipPrimitive.Positioner.Props["side"];
	children: ReactElement;
}) => (
	<TooltipPrimitive.Root
		disabled={content === null || content === undefined || content === false}
	>
		<TooltipPrimitive.Trigger render={children} />
		<TooltipPrimitive.Portal>
			<TooltipPrimitive.Positioner
				side={side}
				sideOffset={6}
				collisionPadding={6}
				className="pointer-events-none z-[140] max-w-(--available-width)"
			>
				<TooltipPrimitive.Popup className="max-w-80 origin-(--transform-origin) rounded-md border bg-popover px-2 py-1 text-[11.5px] leading-snug text-popover-foreground shadow-md transition-[scale,opacity] data-ending-style:scale-98 data-ending-style:opacity-0 data-instant:duration-0 data-starting-style:scale-98 data-starting-style:opacity-0">
					{content}
				</TooltipPrimitive.Popup>
			</TooltipPrimitive.Positioner>
		</TooltipPrimitive.Portal>
	</TooltipPrimitive.Root>
);
