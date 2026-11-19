// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A web page the agent showed with show_visual, drawn as a card on the
// timeline. The page is read from its file, once for the window. Until it's
// read, or when it can't be, the row that names it stays, with Open. A
// visual a newer version replaced folds to one dashed line, and Show
// draws it again.

import { useI18n } from "@maxhq/shared/i18n";

import { useMemo, useState } from "react";
import { useStore } from "zustand";

import { useView } from "@/app/services";
import { VisualRow } from "@/agent-view/items/tools";
import type { ItemOf } from "@/agent-view/items/types";
import { useRow } from "@maxhq/shared/agent-view/context";
import { type VisualOrigin, VisualOriginContext } from "@maxhq/shared/visuals/origin";
import { replacedLine, versionLabel, visualVersions } from "@maxhq/shared/visuals/versions";

import { useVisualPage } from "./pages";
import { VisualMark } from "@/pills/registry";
import { VisualCard } from "./visual-card";

const SHOW =
	"ml-auto h-[20px] shrink-0 cursor-pointer rounded-[5px] px-1.5 text-[11.5px] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

/** A visual's version and whether a newer one replaced it, as one stable value. */
const usePlace = (item: ItemOf<"agent_visual">) => {
	const { timeline } = useRow();
	const place = useStore(timeline, (state) => {
		const versions = visualVersions(state.items);
		return `${versions.version(item.id)}:${versions.replacedBy(item.id) === null ? "" : "replaced"}`;
	});
	const [version, replaced] = place.split(":");
	return { version: Number(version), replaced: replaced === "replaced" };
};

/** The agent's visual on the timeline. */
export const VisualItem = ({ item }: { item: ItemOf<"agent_visual"> }) => {
	const { t } = useI18n();
	const { agent } = useRow();
	const { version, replaced } = usePlace(item);
	const [shown, setShown] = useState(false);
	const folded = replaced && !shown;
	const page = useVisualPage(folded ? null : agent, folded ? null : item.attachment);
	const title = useView((s) => {
		const found = s.view.tables.agent.get(agent);
		return found && found.control !== "observed" ? found.title : null;
	});
	const origin = useMemo<VisualOrigin | null>(
		() => (title === null ? null : { agent: { id: agent, title }, item }),
		[agent, title, item]
	);

	if (folded)
		return (
			<div className="flex h-[28px] min-w-0 items-center gap-2 rounded-[8px] border border-dashed border-border pr-1 pl-3 text-[12px] text-muted-foreground">
				<VisualMark className="text-(--pill-visual-text)" />
				<span className="min-w-0 truncate">{replacedLine(item.title.trim(), version)}</span>
				<button type="button" className={SHOW} onClick={() => setShown(true)}>
					{t("Show")}
				</button>
			</div>
		);
	if (page?.status !== "ready")
		return <VisualRow item={item} note={page?.status === "failed" ? page.error : undefined} />;
	const label = versionLabel(version);
	return (
		<VisualOriginContext value={origin}>
			<VisualCard
				html={page.html}
				title={item.title.trim()}
				version={
					label === null ? undefined : (
						<span className="shrink-0 rounded-[4px] bg-secondary px-1.5 text-[11px] text-muted-foreground tabular-nums">
							{label}
						</span>
					)
				}
			/>
		</VisualOriginContext>
	);
};
