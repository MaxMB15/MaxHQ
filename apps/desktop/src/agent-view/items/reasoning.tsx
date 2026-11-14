// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The agent's thinking, folded into one line that says how long it took.

import { ChevronDown, ChevronRight, Lightbulb } from "lucide-react";

import { formatDuration } from "@maxhq/shared/status/format";

import { useRow } from "@maxhq/shared/agent-view/context";
import { Elapsed } from "../elapsed";
import { isOpen, tookOf } from "@maxhq/shared/agent-view/format";
import { Spinner } from "../icons";
import { useFold } from "@maxhq/shared/agent-view/find-context";
import { Markdown } from "../markdown";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** A reasoning item. */
export const Reasoning = ({ item }: { item: ItemOf<"reasoning"> }) => {
	useI18n();
	const { vendor } = useRow();
	const [open, setOpen] = useFold(`open:${item.id}`, item.id);
	const live = item.streaming || isOpen(item.status);
	const took = tookOf(item);
	const hasText = item.text.trim() !== "";
	const label = live
		? translate("Thinking")
		: took !== null
			? translate("Thought for {{duration}}", { duration: formatDuration(took) })
			: translate("Thought");

	const line = (
		<>
			{live ? (
				<Spinner size={12} className="mx-px" />
			) : (
				<Lightbulb size={14} strokeWidth={1.9} aria-hidden />
			)}
			<span>{label}</span>
			{live && item.started_at !== null ? <Elapsed since={item.started_at} /> : null}
			{hasText ? (
				open ? (
					<ChevronDown size={10} strokeWidth={2.4} aria-hidden />
				) : (
					<ChevronRight size={10} strokeWidth={2.4} aria-hidden />
				)
			) : null}
		</>
	);
	const look =
		"inline-flex h-6 w-fit items-center gap-2 rounded-md pr-2 pl-1.5 text-[12.5px] text-muted-foreground";

	return (
		<div className="flex flex-col gap-1.5">
			{hasText ? (
				<button
					type="button"
					aria-expanded={open}
					onClick={() => setOpen(!open)}
					className={`${look} cursor-pointer hover:bg-accent`}
				>
					{line}
				</button>
			) : (
				<div className={look}>{line}</div>
			)}
			{open && hasText ? (
				<div className="ml-[13px] flex flex-col gap-2 border-l-2 border-track pl-4">
					<Markdown
						text={item.text}
						streaming={item.streaming}
						className="text-[12.5px] text-muted-foreground"
					/>
					{vendor === "codex" ? (
						<p className="text-[11px] text-subtle-foreground">
							{translate("Summary from Codex. The full reasoning isn't shared.")}
						</p>
					) : null}
				</div>
			) : null}
		</div>
	);
};
