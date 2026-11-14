// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A link to a web page. The app opens every page link in the default
// browser, never in its own window.

import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** A link that opens `href` in the browser, showing the address unless given words. */
export const OutLink = ({
	href,
	children,
	className,
}: {
	href: string;
	children?: ReactNode;
	className?: string;
}) => (
	<a
		href={href}
		target="_blank"
		rel="noreferrer"
		className={cn("text-primary hover:underline", className)}
	>
		{children ?? href}
	</a>
);
