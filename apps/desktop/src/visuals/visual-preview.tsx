// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual drawn small, for a pill's preview: the live page laid out at a
// card's width and scaled down, so it looks as it does in the chat. It
// takes no clicks, since a preview is only to look at.

import { useI18n } from "@maxhq/shared/i18n";

import type { AgentId, AttachmentRef } from "@maxhq/shared/bindings";
import type { PageState } from "@maxhq/shared/visuals/pages";

import { useVisualPage } from "./pages";
import { VisualFrame } from "./visual-frame";

/** The width the page lays out at before it's scaled. */
const LAID_OUT_PX = 560;
const SCALE = 0.5;
/** How tall the preview gets, scaled. */
const MOST_HEIGHT_PX = 180;

/** A page, live and scaled down. */
export const ScaledVisual = ({ html, title }: { html: string; title: string }) => (
	<div
		className="pointer-events-none mb-2 overflow-hidden rounded-md border border-border bg-background"
		style={{ width: LAID_OUT_PX * SCALE, maxHeight: MOST_HEIGHT_PX }}
		inert
	>
		<div
			style={{
				width: LAID_OUT_PX,
				transform: `scale(${SCALE})`,
				transformOrigin: "top left",
			}}
		>
			<VisualFrame html={html} title={title} />
		</div>
	</div>
);

/** A page read from its file through `agent`, scaled down. It reads once it's drawn. */
export const ReadVisualPage = ({
	agent,
	file,
	title,
}: {
	agent: AgentId;
	file: AttachmentRef;
	title: string;
}) => <PagePreview state={useVisualPage(agent, file)} title={title} />;

/** A page that may still be on its way, scaled down once it's here. */
export const PagePreview = ({ state, title }: { state: PageState | undefined; title: string }) => {
	const { t } = useI18n();
	if (state?.status === "ready") return <ScaledVisual html={state.html} title={title} />;
	if (state?.status === "failed")
		return <div className="mb-2 text-status-error-text">{state.error}</div>;
	return <div className="mb-2 text-muted-foreground">{t("Reading the page")}</div>;
};
