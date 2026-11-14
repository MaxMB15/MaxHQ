// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as arrow
// functions. The scroll fade and the virtualized helper are left out, and the
// thumb takes MaxHQ's scrollbar colors.

import { ScrollArea as ScrollAreaPrimitive } from "@base-ui/react/scroll-area";

import { cn } from "@/lib/utils";

/** A thin scrollbar that shows while you scroll or hover. */
export const ScrollBar = ({
	className,
	orientation = "vertical",
	...props
}: ScrollAreaPrimitive.Scrollbar.Props) => (
	<ScrollAreaPrimitive.Scrollbar
		className={cn(
			"flex opacity-0 transition-opacity delay-300 data-hovering:opacity-100 data-hovering:delay-0 data-hovering:duration-100 data-scrolling:opacity-100 data-scrolling:delay-0 data-scrolling:duration-100 data-[orientation=horizontal]:mx-1 data-[orientation=horizontal]:mb-px data-[orientation=horizontal]:h-1.5 data-[orientation=horizontal]:flex-col data-[orientation=vertical]:my-1 data-[orientation=vertical]:mr-px data-[orientation=vertical]:w-1.5",
			className
		)}
		data-slot="scroll-area-scrollbar"
		orientation={orientation}
		{...props}
	>
		<ScrollAreaPrimitive.Thumb
			className="relative flex-1 rounded-full bg-scrollbar-thumb transition-colors hover:bg-scrollbar-thumb-hover"
			data-slot="scroll-area-thumb"
		/>
	</ScrollAreaPrimitive.Scrollbar>
);

/**
 * A box that scrolls its content up and down, with the scrollbar above.
 * It never scrolls sideways, so a row too wide can't slide the whole list
 * out of sight. Base UI sets the viewport to scroll both ways, which only
 * an important rule overrides.
 */
export const ScrollArea = ({
	className,
	viewportClassName,
	children,
	...props
}: ScrollAreaPrimitive.Root.Props & {
	/** Classes for the scrolling element, such as its padding. */
	viewportClassName?: string;
}) => (
	<ScrollAreaPrimitive.Root
		className={cn("relative min-h-0 overflow-hidden", className)}
		{...props}
	>
		<ScrollAreaPrimitive.Viewport
			className={cn(
				"h-full max-h-[inherit] overflow-x-hidden! overscroll-contain outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset",
				viewportClassName
			)}
			data-slot="scroll-area-viewport"
		>
			{children}
		</ScrollAreaPrimitive.Viewport>
		<ScrollBar orientation="vertical" />
	</ScrollAreaPrimitive.Root>
);
