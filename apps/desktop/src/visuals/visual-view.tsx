// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual in a tab of its own, the view for mockups. The bar over it sets
// the frame's width to a phone's, a tablet's or the pane's, and a zoom that
// lays the page out again. Source swaps in the page as code, and Copy, Save
// as .html and the More menu do what the card's do. The frame fills the
// pane and the page scrolls inside it.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useEffect, useMemo, useState } from "react";

import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";
import { inApp } from "@/native/platform";
import { CodeLines } from "@/viewer/code-file";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import type { VisualTransfer } from "@maxhq/shared/visuals/transfer";
import { visualBytes, visualFileName } from "@maxhq/shared/visuals/visual";

import { saveVisual, useVisualContextMenu, VisualMenu, visualActions } from "./actions";
import { visualPage } from "./tab-pages";
import { VisualFrame } from "./visual-frame";

/** The widths the frame can take, in CSS pixels, or null to fill the pane. */
export const VISUAL_WIDTHS = [
	{
		get label() {
			return t("Phone {{width}}", { width: 390 });
		},
		width: 390,
	},
	{
		get label() {
			return t("Tablet {{width}}", { width: 768 });
		},
		width: 768,
	},
	{
		get label() {
			return t("Fit");
		},
		width: null,
	},
] as const;

/** The zooms the view offers, as a browser's do. */
export const VISUAL_ZOOMS = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const;

const percent = (zoom: number) =>
	formatNumber(zoom, { style: "percent", maximumFractionDigits: 0 });

const BUTTON =
	"inline-flex h-[24px] shrink-0 items-center gap-1 rounded-[6px] border border-border px-2 text-[12px] text-secondary-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring aria-pressed:bg-selected aria-pressed:text-foreground";

/** A visual filling a pane, with its bar. */
export const VisualView = ({ visual }: { visual: VisualTransfer }) => {
	const { t } = useI18n();
	const { html, title, from } = visual;
	const [width, setWidth] = useState<number | null>(null);
	const [zoom, setZoom] = useState(1);
	const [source, setSource] = useState(false);
	const [runs, setRuns] = useState(0);
	const actions = useMemo(
		() => visualActions({ html, title }, { reload: () => setRuns((n) => n + 1) }),
		[html, title]
	);
	const onContextMenu = useVisualContextMenu(actions);
	const fence = useMemo(() => toVisualFence({ html, title }), [html, title]);
	const lines = useMemo(() => (source ? html.split("\n") : []), [source, html]);
	const fill = useMemo(() => ({ zoom }), [zoom]);

	return (
		<div className="flex h-full flex-col bg-background" onContextMenu={onContextMenu}>
			<div className="flex min-h-[40px] shrink-0 flex-wrap items-center gap-x-1.5 gap-y-1 border-b border-border px-3 py-1.5">
				<span aria-hidden className="text-(--pill-visual-text)">
					◆
				</span>
				<h2 className="min-w-0 truncate text-[13px] font-semibold text-foreground">
					{title || t("Visual")}
				</h2>
				{from ? (
					<span className="min-w-0 truncate text-[12px] text-muted-foreground">
						{t("from {{from}}", { from })}
					</span>
				) : null}
				<span className="flex-1" />
				{VISUAL_WIDTHS.map((preset) => (
					<button
						key={preset.label}
						type="button"
						aria-pressed={!source && width === preset.width}
						disabled={source}
						onClick={() => setWidth(preset.width)}
						className={cn(BUTTON, "disabled:opacity-50")}
					>
						{preset.label}
					</button>
				))}
				<select
					aria-label={t("Zoom")}
					value={zoom}
					disabled={source}
					onChange={(event) => setZoom(Number(event.target.value))}
					className={cn(BUTTON, "bg-background disabled:opacity-50")}
				>
					{VISUAL_ZOOMS.map((step) => (
						<option key={step} value={step}>
							{percent(step)}
						</option>
					))}
				</select>
				<button
					type="button"
					aria-pressed={source}
					onClick={() => setSource((shown) => !shown)}
					className={BUTTON}
				>
					&lt;/&gt; {t("Source")}
				</button>
				<CopyButton
					text={fence}
					icon={false}
					title={t("Copy the visual")}
					className={BUTTON}
				/>
				{inApp() ? (
					<button
						type="button"
						onClick={() => saveVisual({ html, title })}
						className={BUTTON}
					>
						{t("Save as .html…")}
					</button>
				) : null}
				<VisualMenu actions={actions} />
			</div>
			<div className="min-h-0 flex-1">
				{source ? (
					// No agent's file, so no line is ever asked for.
					<CodeLines
						agent=""
						path={visualFileName(title)}
						lines={lines}
						bytes={visualBytes(html)}
					/>
				) : (
					<div
						data-visual-box
						className={cn(
							"mx-auto h-full max-w-full",
							width !== null && "border-x border-border"
						)}
						style={{ width: width ?? "100%" }}
					>
						<VisualFrame
							html={html}
							title={title}
							runs={runs}
							fill={fill}
							className="flex h-full flex-col"
						/>
					</div>
				)}
			</div>
		</div>
	);
};

/**
 * What a visual's pane shows: the page kept under `pageKey`. A pane whose
 * page this window doesn't have, as after a restart, closes.
 */
export const VisualPaneFor = ({ pageKey, close }: { pageKey: string; close: () => void }) => {
	const visual = visualPage(pageKey);
	useEffect(() => {
		if (!visual) close();
	}, [visual, close]);
	return visual ? <VisualView visual={visual} /> : null;
};
