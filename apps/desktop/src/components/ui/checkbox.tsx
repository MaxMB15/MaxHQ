// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as an
// arrow function. The mixed state and the light theme's shadows are left out.

import { Checkbox as CheckboxPrimitive } from "@base-ui/react/checkbox";

import { cn } from "@/lib/utils";

/** A tick box. Name it with `aria-label`. */
export const Checkbox = ({ className, ...props }: CheckboxPrimitive.Root.Props) => (
	<CheckboxPrimitive.Root
		className={cn(
			"relative inline-flex size-4 shrink-0 items-center justify-center rounded-[.25rem] border border-input bg-input/32 outline-none transition-shadow focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring data-checked:border-primary data-disabled:opacity-60",
			className
		)}
		data-slot="checkbox"
		{...props}
	>
		<CheckboxPrimitive.Indicator
			className="absolute -inset-px flex items-center justify-center rounded-[.25rem] text-primary-foreground data-checked:bg-primary data-unchecked:hidden"
			data-slot="checkbox-indicator"
		>
			<svg
				aria-hidden
				className="size-3"
				fill="none"
				stroke="currentColor"
				strokeLinecap="round"
				strokeLinejoin="round"
				strokeWidth="3"
				viewBox="0 0 24 24"
			>
				<path d="M5.252 12.7 10.2 18.63 18.748 5.37" />
			</svg>
		</CheckboxPrimitive.Indicator>
	</CheckboxPrimitive.Root>
);
