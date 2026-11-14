// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: the find bar at the
// top right of the conversation, with the query, where the active match sits
// among all of them, and buttons to move between them. The rest is MaxHQ's:
// the bar reads its agent view's find, Escape closes it from anywhere in it,
// and closing hands focus back to where it was before the bar took it.

import { useEffect, useRef, type KeyboardEvent } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { useStore } from "zustand";

import { MAX_FIND_QUERY } from "@maxhq/shared/find/search";
import { type Find, findLabel } from "@maxhq/shared/find/store";

import { cn } from "@/lib/utils";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ICON_BUTTON =
	"grid size-6 shrink-0 cursor-pointer place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-foreground";

/** The find bar of one agent view, shown while its find is open. */
export const FindBar = ({ find }: { find: Find }) => {
	useI18n();
	const state = useStore(find.store);
	const input = useRef<HTMLInputElement>(null);
	const bar = useRef<HTMLDivElement>(null);
	// What had focus before the bar took it, to hand it back on close.
	const before = useRef<HTMLElement | null>(null);

	// Each find shortcut bumps `focus`, so an open bar takes focus and picks its query again.
	useEffect(() => {
		if (!state.open) return;
		const active = document.activeElement;
		if (active instanceof HTMLElement && !bar.current?.contains(active))
			before.current = active;
		input.current?.focus();
		input.current?.select();
	}, [state.open, state.focus]);

	if (!state.open) return null;

	const close = () => {
		const back = before.current;
		before.current = null;
		find.close();
		if (back?.isConnected) back.focus();
	};

	const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
		if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
		if (event.key === "Escape") {
			event.preventDefault();
			event.stopPropagation();
			close();
			return;
		}
		if (event.key === "Enter" && event.target === input.current) {
			event.preventDefault();
			event.stopPropagation();
			find.step(event.shiftKey ? -1 : 1);
		}
	};

	const label = findLabel(state);
	const stuck = state.result === null || state.result.total === 0;

	return (
		<div
			ref={bar}
			role="search"
			aria-label={translate("Find in conversation")}
			aria-busy={state.status === "searching"}
			onKeyDown={onKeyDown}
			className="absolute top-2 right-3 z-20 flex h-8 w-[min(340px,calc(100%-1.5rem))] items-center gap-1 rounded-lg border border-input bg-popover pr-1 pl-2.5 shadow-md"
		>
			<input
				ref={input}
				type="text"
				value={state.typed}
				aria-label={translate("Find in conversation")}
				placeholder={translate("Find in conversation")}
				maxLength={MAX_FIND_QUERY}
				spellCheck={false}
				autoComplete="off"
				onChange={(event) => find.type(event.target.value)}
				className="h-full min-w-0 flex-1 bg-transparent text-[13px] text-foreground outline-none placeholder:text-muted-foreground"
			/>
			<span
				aria-live="polite"
				className={cn(
					"shrink-0 px-1 text-center text-xs whitespace-nowrap tabular-nums",
					label.none ? "text-destructive" : "text-muted-foreground"
				)}
			>
				{label.text}
			</span>
			{state.status === "failed" ? (
				<button
					type="button"
					onClick={find.retry}
					className="shrink-0 cursor-pointer rounded-[5px] px-1.5 text-xs text-foreground hover:bg-border focus-visible:ring-2 focus-visible:ring-ring"
				>
					{translate("Retry")}
				</button>
			) : null}
			<button
				type="button"
				aria-label={translate("Previous match")}
				disabled={stuck}
				onClick={() => find.step(-1)}
				className={ICON_BUTTON}
			>
				<ChevronUp aria-hidden size={14} strokeWidth={2.2} />
			</button>
			<button
				type="button"
				aria-label={translate("Next match")}
				disabled={stuck}
				onClick={() => find.step(1)}
				className={ICON_BUTTON}
			>
				<ChevronDown aria-hidden size={14} strokeWidth={2.2} />
			</button>
			<button
				type="button"
				aria-label={translate("Close find")}
				onClick={close}
				className={ICON_BUTTON}
			>
				<X aria-hidden size={13} strokeWidth={2.2} />
			</button>
		</div>
	);
};
