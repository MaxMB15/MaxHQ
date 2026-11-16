// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual in a message or a note: a card with its title over the visual's
// frame. Copy puts the visual on the clipboard as its fenced block, which
// pastes back as the same visual, and the More menu and a right click offer
// the rest, Add to chat and Add to note among it. The title row drags the
// visual to a composer, a note or an agent in the sidebar. Tell the agent
// quotes the page's errors into the composer of the agent whose reply holds
// it. Find and quotes leave the card out.

import { useI18n } from "@maxhq/shared/i18n";

import { type ReactNode, useMemo, useState, useSyncExternalStore } from "react";

import { type Services, useServicesIfAny } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { panesOf } from "@/layout/panes";
import { cn } from "@/lib/utils";
import { makeQuote } from "@maxhq/shared/pills/quote";
import { toVisualFence } from "@maxhq/shared/visuals/fence";

import { useVisualContextMenu, VisualMenu, visualActions } from "./actions";
import { useAddToNote } from "./add-to-note";
import { agentFrom, chatActions, copyVisual, dragVisual } from "./carry";
import { useVisualOrigin, type VisualOrigin } from "./origin";
import { type VisualError, VisualFrame } from "./visual-frame";

const HOVER_ACTIONS =
	"flex items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/visual:opacity-100 focus-within:opacity-100 has-[[data-popup-open]]:opacity-100 pointer-coarse:opacity-100";

const COPY_LOOK =
	"inline-flex h-[22px] items-center gap-1.5 rounded-[5px] px-[7px] text-[11.5px] text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring";

/** What Tell the agent puts in the quote: the errors, each on its own line. */
export const errorsQuote = (
	title: string,
	errors: readonly VisualError[],
	count: number
): string => {
	const lines = errors.map(({ message, line }) =>
		line === null ? message : `Line ${line}: ${message}`
	);
	const more = count - errors.length;
	return [
		`The visual${title ? ` "${title}"` : ""} showed ${count === 1 ? "an error" : `${count} errors`}:`,
		...lines,
		...(more > 0 ? [`and ${more} more`] : []),
	].join("\n");
};

const tell =
	(origin: VisualOrigin, html: string, title: string) =>
	(errors: readonly VisualError[], count: number) => {
		const at = origin.item.kind === "assistant_message" ? origin.item.text.indexOf(html) : -1;
		const quote = makeQuote(origin.agent, origin.item, {
			text: errorsQuote(title, errors, count),
			position: Math.max(0, at),
		});
		quoteInbox.send(origin.agent.id, quote, true);
	};

const NO_CHANGES = () => () => {};

/**
 * Where a reply's visual came from, as its pill names it. A card drawn
 * without the app's services, such as a note's editor on its own, has none.
 */
const useOriginFrom = (services: Services | null, origin: VisualOrigin | null): string | null =>
	useSyncExternalStore(services?.view.subscribe ?? NO_CHANGES, () =>
		services && origin ? agentFrom(services.view.getState().view, origin.agent.id) : null
	);

/** A visual block in a message or a note. */
export const VisualCard = ({
	html,
	title,
	version,
	from,
	buttons,
	className,
}: {
	html: string;
	title: string;
	/** Which version of the visual this is, when it has others. */
	version?: ReactNode;
	/** Where it came from, for its pill. The reply's agent unless given. */
	from?: string | null;
	/** More buttons for the title row, before Copy. */
	buttons?: ReactNode;
	className?: string;
}) => {
	const { t } = useI18n();
	const origin = useVisualOrigin();
	const services = useServicesIfAny();
	const originFrom = useOriginFrom(services, origin);
	const visual = useMemo(
		() => ({ html, title, from: from === undefined ? originFrom : from }),
		[html, title, from, originFrom]
	);
	const [runs, setRuns] = useState(0);
	const addToNote = useAddToNote(html, title);
	const actions = useMemo(
		() =>
			visualActions(visual, {
				reload: () => setRuns((n) => n + 1),
				open: services ? () => panesOf(services).openVisual(visual) : undefined,
				carry: [
					...(services ? chatActions(services, visual, origin?.agent.id ?? null) : []),
					...(addToNote ? [addToNote] : []),
				],
			}),
		[visual, services, origin, addToNote]
	);
	const write = useMemo(() => () => copyVisual(visual), [visual]);
	const onContextMenu = useVisualContextMenu(actions);
	const fence = useMemo(() => toVisualFence({ html, title }), [html, title]);
	const onTell = useMemo(
		() => (origin ? tell(origin, html, title) : undefined),
		[origin, html, title]
	);
	return (
		<figure
			data-find-skip
			aria-label={title || t("Visual")}
			onContextMenu={onContextMenu}
			className={cn(
				"group/visual overflow-hidden rounded-[10px] border border-border bg-background",
				className
			)}
		>
			<figcaption
				draggable
				onDragStart={(event) => dragVisual(event, visual)}
				className="flex h-[34px] cursor-grab items-center gap-2 border-b border-border pr-1.5 pl-3 text-[12px] text-muted-foreground select-none active:cursor-grabbing"
			>
				<span aria-hidden className="-mr-0.5 tracking-[-2px] text-subtle-foreground">
					⋮⋮
				</span>
				<span aria-hidden className="text-(--pill-visual-text)">
					◆
				</span>
				<span className="min-w-0 truncate text-[13px] font-medium text-foreground">
					{title || t("Visual")}
				</span>
				{version}
				<span className="flex-1" />
				<span className={HOVER_ACTIONS}>
					{buttons}
					<CopyButton
						text={fence}
						write={write}
						title={t("Copy the visual")}
						className={COPY_LOOK}
					/>
					<VisualMenu actions={actions} />
				</span>
			</figcaption>
			<VisualFrame html={html} title={title} runs={runs} onTell={onTell} />
		</figure>
	);
};
