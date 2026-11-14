// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The window's visual pages, read from their files through the media
// cache. A card on the timeline and a sent pill's preview read a page here,
// so each page is read once however many places show it.

import { t } from "@maxhq/shared/i18n";

import type { DataSource } from "@maxhq/shared/data/source";
import type { AgentId, AttachmentRef } from "@maxhq/shared/bindings";
import { errorText } from "@maxhq/shared/timeline/store";
import { MAX_VISUAL_BYTES } from "@maxhq/shared/visuals/visual";
import {
	createVisualPages,
	type PageReader,
	type PageState,
	type VisualPages,
} from "@maxhq/shared/visuals/pages";
import { useEffect, useSyncExternalStore } from "react";

import { useServices } from "@/app/services";
import { formatBytes } from "@/composer/attachments";
import { readBytes } from "@/viewer/load";

const noProgress = () => {};

/** Reads a page's file through the agent that has it. */
export const pageReader =
	(source: Pick<DataSource, "media">, read: typeof readBytes = readBytes): PageReader =>
	async (agent, file) => {
		const result = await source.media(agent, file.path, MAX_VISUAL_BYTES, false, noProgress);
		if (result.status === "error") throw new Error(errorText(result.error));
		const media = result.data;
		switch (media.kind) {
			case "cached":
				return new TextDecoder().decode(await read(media.url, media.size));
			case "too_large":
				throw new Error(
					t("The page is {{size}}, more than the {{most}} a visual can be.", {
						size: formatBytes(media.size),
						most: formatBytes(MAX_VISUAL_BYTES),
					})
				);
			case "refused":
				throw new Error(media.message);
		}
	};

const windows = new WeakMap<Pick<DataSource, "media">, VisualPages>();

/** The pages read through a source, one cache for each. */
export const pagesOf = (source: Pick<DataSource, "media">): VisualPages => {
	let pages = windows.get(source);
	if (!pages) {
		pages = createVisualPages(pageReader(source));
		windows.set(source, pages);
	}
	return pages;
};

/** This window's pages. */
export const useVisualPages = (): VisualPages => pagesOf(useServices().source);

/**
 * A page's state, read through `agent` when nothing has it yet. Null
 * `file` reads nothing.
 */
export const useVisualPage = (
	agent: AgentId | null,
	file: AttachmentRef | null
): PageState | undefined => {
	const pages = useVisualPages();
	const state = useSyncExternalStore(pages.subscribe, () =>
		file === null ? undefined : pages.get(file.id)
	);
	useEffect(() => {
		if (agent !== null && file !== null) pages.load(agent, file);
	}, [agent, file, pages]);
	return state;
};
