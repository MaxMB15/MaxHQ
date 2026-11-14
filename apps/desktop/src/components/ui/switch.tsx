// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as an
// arrow function. The mixed state, the small size and the squash on press are
// left out, and the thumb takes MaxHQ's colors.

import { Switch as SwitchPrimitive } from "@base-ui/react/switch";

import { cn } from "@/lib/utils";

/** An on and off switch. Name it with `aria-label`. */
export const Switch = ({ className, ...props }: SwitchPrimitive.Root.Props) => (
	<SwitchPrimitive.Root
		className={cn(
			"inline-flex h-[calc(var(--thumb-size)+2px)] w-[calc(var(--thumb-size)*2-2px)] shrink-0 cursor-pointer items-center rounded-full p-[2px] outline-none transition-[background-color,box-shadow] duration-200 [--thumb-size:--spacing(4)] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background data-checked:bg-primary data-unchecked:bg-input data-disabled:cursor-not-allowed data-disabled:opacity-60",
			className
		)}
		data-slot="switch"
		{...props}
	>
		<SwitchPrimitive.Thumb
			className="pointer-events-none block size-[calc(var(--thumb-size)-2px)] shrink-0 rounded-(--thumb-size) bg-foreground-strong shadow-sm [transition:translate_.15s] data-checked:translate-x-[calc(var(--thumb-size)-4px)] data-unchecked:bg-muted-foreground"
			data-slot="switch-thumb"
		/>
	</SwitchPrimitive.Root>
);
