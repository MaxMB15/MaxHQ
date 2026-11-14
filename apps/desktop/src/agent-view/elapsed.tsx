// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A running clock for a step that hasn't ended. It writes its own text once
// a second, so a hundred running rows don't render a hundred times. This
// follows MIT-licensed code, see THIRD_PARTY_NOTICES.

import { useLayoutEffect, useRef } from "react";

import type { Timestamp } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { formatDuration } from "@maxhq/shared/status/format";

/** Time since `since`, such as "12 s" or "1 min 12 s", kept current. */
export const Elapsed = ({
	since,
	format = formatDuration,
	className,
}: {
	since: Timestamp;
	format?: (ms: number) => string;
	className?: string;
}) => {
	const ref = useRef<HTMLSpanElement>(null);

	useLayoutEffect(() => {
		const update = () => {
			const el = ref.current;
			if (!el) return;
			const text = format(Date.now() - since);
			if (el.textContent !== text) el.textContent = text;
		};
		update();
		const timer = setInterval(update, 1000);
		return () => clearInterval(timer);
	}, [since, format]);

	return <span ref={ref} className={cn("tabular-nums", className)} />;
};
