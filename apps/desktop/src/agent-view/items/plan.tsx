// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A plan the agent wrote, and its to-do list with a bar for each step.

import { CircleCheck, ListChecks, ListTodo } from "lucide-react";

import type { TodoState } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";

import { Spinner } from "../icons";
import { Markdown } from "../markdown";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** A plan the agent proposed, as Markdown. */
export const PlanCard = ({ item }: { item: ItemOf<"plan"> }) => {
	useI18n();
	return (
		<section
			aria-label={translate("Proposed plan")}
			className="flex flex-col gap-2 rounded-lg border border-border bg-subpanel px-3 pt-2.5 pb-3"
		>
			<header className="flex items-center gap-2 text-[12.5px]">
				<ListTodo
					size={14}
					strokeWidth={1.9}
					aria-hidden
					className="text-muted-foreground"
				/>
				<span className="font-semibold text-foreground">{translate("Proposed plan")}</span>
				{item.streaming ? (
					<span className="ml-auto inline-flex items-center gap-1.5 text-[11.5px] text-muted-foreground">
						<Spinner size={10} />
						{translate("Writing")}
					</span>
				) : null}
			</header>
			<Markdown text={item.markdown} streaming={item.streaming} />
		</section>
	);
};

const BAR: Record<TodoState, string> = {
	done: "bg-status-ready",
	in_progress: "bg-status-ready/40",
	pending: "bg-track",
};

const STATE_LABEL: Record<TodoState, string> = {
	get done() {
		return translate("Done");
	},
	get in_progress() {
		return translate("In progress");
	},
	get pending() {
		return translate("Not started");
	},
};

const Mark = ({ state }: { state: TodoState }) => {
	useI18n();
	if (state === "done") {
		return (
			<CircleCheck
				size={14}
				strokeWidth={1.9}
				className="shrink-0 text-status-ready"
				aria-label={STATE_LABEL[state]}
			/>
		);
	}
	return (
		<span
			role="img"
			aria-label={STATE_LABEL[state]}
			className="grid size-3.5 shrink-0 place-items-center"
		>
			{state === "in_progress" ? (
				<Spinner size={11} />
			) : (
				<span className="size-[11px] rounded-full border-[1.5px] border-subtle-foreground" />
			)}
		</span>
	);
};

/** The agent's to-do list. */
export const TodosCard = ({ item }: { item: ItemOf<"todos"> }) => {
	useI18n();
	const done = item.todos.filter((t) => t.state === "done").length;
	return (
		<section
			aria-label={translate("Plan")}
			className="flex flex-col gap-1 rounded-lg border border-border bg-subpanel px-3 pt-2.5 pb-2"
		>
			<header className="flex items-center gap-2 pb-1 text-[12.5px]">
				<ListChecks
					size={14}
					strokeWidth={1.9}
					aria-hidden
					className="text-muted-foreground"
				/>
				<span className="font-semibold text-foreground">{translate("Plan")}</span>
				<span className="text-muted-foreground tabular-nums">
					{done} {translate(" of ")}
					{item.todos.length} {translate(" done")}
				</span>
				<span className="ml-auto flex gap-[3px]" aria-hidden>
					{item.todos.map((todo, i) => (
						// The list is replaced whole on each change, so a step's place is its identity.
						<span
							key={i}
							className={cn("h-1 w-[22px] rounded-[2px]", BAR[todo.state])}
						/>
					))}
				</span>
			</header>
			<ul className="flex flex-col">
				{item.todos.map((todo, i) => (
					<li key={i} className="flex min-h-[22px] items-center gap-[9px] text-[12.5px]">
						<Mark state={todo.state} />
						<span
							className={cn(
								"min-w-0 break-words select-text",
								todo.state === "done" && "text-muted-foreground",
								todo.state === "in_progress" && "text-foreground-strong",
								todo.state === "pending" && "text-secondary-foreground"
							)}
						>
							{todo.text}
						</span>
					</li>
				))}
			</ul>
		</section>
	);
};
