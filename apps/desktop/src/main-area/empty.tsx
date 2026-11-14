// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

/** A line centered in the main area or a pane. */
export const Empty = ({ children }: { children: string }) => (
	<div className="grid h-full place-items-center px-8">
		<p className="max-w-[48ch] text-center text-[13px] text-muted-foreground">{children}</p>
	</div>
);
