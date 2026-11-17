// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A place a live terminal shows in. The view itself lives in the window's
// registry, so this only lends it a host while mounted.

import { useEffect, useRef } from "react";

import { useServices } from "@/app/services";
import type { TerminalId } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";

import { registryOf } from "./live";

/** One terminal's live output, filling its parent or as tall as `maxRows` lets it. */
export const LiveTerminalHost = ({
	terminal,
	maxRows,
	interactive,
	className,
}: {
	terminal: TerminalId;
	maxRows?: number;
	interactive: boolean;
	className?: string;
}) => {
	const services = useServices();
	const host = useRef<HTMLDivElement>(null);
	useEffect(() => {
		const registry = registryOf(services);
		if (!registry || !host.current) return;
		return registry.place(terminal, host.current, { interactive, maxRows });
	}, [services, terminal, interactive, maxRows]);
	return (
		<div
			ref={host}
			data-terminal={terminal}
			className={cn(
				"min-w-0 overflow-hidden bg-(--terminal-background)",
				maxRows === undefined && "h-full",
				className
			)}
		/>
	);
};
