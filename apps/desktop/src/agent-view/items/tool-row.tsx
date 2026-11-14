// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one-line row every tool step shares: an icon, a verb, what it acted
// on, and a figure at the end. A row with more to show opens below itself.

import { ChevronDown, ChevronRight } from "lucide-react";
import type { ReactNode } from "react";

import type { Timestamp } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";

import { Elapsed } from "../elapsed";
import { formatTook } from "@maxhq/shared/agent-view/format";
import { Spinner } from "../icons";

/** A path, a query or a command in a row, cut short to fit. */
export const Target = ({
	children,
	className,
	title,
	find = false,
}: {
	children: ReactNode;
	className?: string;
	title?: string;
	/** Set when find counts its text. */
	find?: boolean;
}) => (
	<span
		title={title}
		data-find={find ? "plain" : undefined}
		className={cn(
			"min-w-0 truncate font-mono text-[11.5px] text-terminal select-text",
			className
		)}
	>
		{children}
	</span>
);

/** The box a row's details open in, under the verb. */
export const Detail = ({ children, className }: { children: ReactNode; className?: string }) => (
	<div
		className={cn(
			"ml-7 overflow-hidden rounded-md border border-border bg-subpanel select-text",
			className
		)}
	>
		{children}
	</div>
);

/** A tool step's row. */
export const ToolRow = ({
	icon,
	verb,
	target,
	meta,
	since,
	running = false,
	open,
	onToggle,
	action,
	children,
}: {
	icon: ReactNode;
	verb: ReactNode;
	target?: ReactNode;
	/** The figure at the end, such as "214 lines" or "Exit 0, 7.1 s". */
	meta?: ReactNode;
	/** When it started, for the clock a running step shows. */
	since?: Timestamp | null;
	running?: boolean;
	/** Whether the details show. Leave unset for a row with none. */
	open?: boolean;
	onToggle?: () => void;
	/** A control beside the row, such as a download button. */
	action?: ReactNode;
	/** The details, shown while open. */
	children?: ReactNode;
}) => {
	const expandable = open !== undefined && onToggle !== undefined;
	const line = (
		<>
			<span className="grid size-3.5 shrink-0 place-items-center" aria-hidden>
				{running ? <Spinner size={12} /> : icon}
			</span>
			<span className="shrink-0 text-secondary-foreground">{verb}</span>
			{target}
			<span className="ml-auto shrink-0 pl-2 text-[11.5px] whitespace-nowrap tabular-nums">
				{running && since != null ? (
					<Elapsed since={since} format={formatTook} className="text-status-busy-text" />
				) : (
					meta
				)}
			</span>
			{expandable ? (
				open ? (
					<ChevronDown size={10} strokeWidth={2.4} aria-hidden className="shrink-0" />
				) : (
					<ChevronRight size={10} strokeWidth={2.4} aria-hidden className="shrink-0" />
				)
			) : null}
		</>
	);
	const row =
		"flex h-[26px] w-full min-w-0 items-center gap-2 rounded-md pr-2 pl-1.5 text-left text-[12.5px] text-muted-foreground";
	const head = expandable ? (
		<button
			type="button"
			aria-expanded={open}
			onClick={onToggle}
			className={cn(row, "cursor-pointer hover:bg-accent")}
		>
			{line}
		</button>
	) : (
		<div className={row}>{line}</div>
	);
	return (
		<div className="flex flex-col gap-1">
			{action ? (
				<div className="flex min-w-0 items-center gap-0.5">
					{head}
					{action}
				</div>
			) : (
				head
			)}
			{expandable && open ? children : null}
		</div>
	);
};
