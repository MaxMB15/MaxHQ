// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The vendor marks and the small spinner the timeline uses.

import type { ReactNode } from "react";

import type { Vendor } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";

const MARK: Record<Vendor, ReactNode> = {
	claude: (
		<path d="M12 3v6M12 15v6M3 12h6M15 12h6M5.6 5.6l4.3 4.3M14.1 14.1l4.3 4.3M18.4 5.6l-4.3 4.3M9.9 14.1l-4.3 4.3" />
	),
	codex: (
		<>
			<path d="M12 3l7.8 4.5v9L12 21l-7.8-4.5v-9z" />
			<path d="M10 9.5L7.5 12l2.5 2.5M14 9.5l2.5 2.5-2.5 2.5" />
		</>
	),
	cursor: <path d="M5 3.5l13.5 7.2-6 1.6-2.6 5.7z" />,
	acp: (
		<>
			<rect x="4" y="7" width="16" height="12" rx="2" />
			<path d="M12 3v4M9 13h.01M15 13h.01" />
		</>
	),
};

/** Claude's asterisk, Codex's hexagon, Cursor's pointer or another agent's robot, drawn in the current color. */
export const VendorIcon = ({
	vendor,
	size = 12,
	className,
}: {
	vendor: Vendor;
	size?: number;
	className?: string;
}) => (
	<svg
		aria-hidden="true"
		width={size}
		height={size}
		viewBox="0 0 24 24"
		fill="none"
		stroke="currentColor"
		strokeLinecap="round"
		strokeLinejoin="round"
		strokeWidth={vendor === "claude" ? 2.4 : 2}
		className={cn("shrink-0", className)}
	>
		{MARK[vendor]}
	</svg>
);

/** A ring that turns while a step runs. */
export const Spinner = ({ size = 12, className }: { size?: number; className?: string }) => (
	<span
		aria-hidden="true"
		className={cn(
			"inline-block shrink-0 animate-busy rounded-full border-2 border-status-busy/25 border-t-status-busy",
			className
		)}
		style={{ width: size, height: size }}
	/>
);
