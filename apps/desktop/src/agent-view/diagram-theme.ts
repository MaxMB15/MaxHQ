// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a Mermaid diagram takes the app's look. Words and lines use the app's
// own tokens. A diagram's series use the `--series-*` tokens in a fixed order,
// each theme's checked against the panel they sit on, `--subpanel`, for colour
// blindness and for telling neighbours apart, so a series always gets the same
// colour for its place, and the status colours keep their meanings.

import type { Theme } from "@maxhq/shared/theme/theme";

const SERIES_COUNT = 8;

const FONT = 'ui-sans-serif, -apple-system, "SF Pro Text", "Segoe UI", sans-serif';

/** Mermaid's theme settings, in the app's colours as they are now. */
export const diagramTheme = (
	token: (name: string) => string,
	theme: Theme
): Record<string, unknown> => {
	const series = Array.from({ length: SERIES_COUNT }, (_, i) => token(`--series-${i}`));
	const each = (name: string, value: (color: string) => string) =>
		Object.fromEntries(series.map((color, i) => [`${name}${i}`, value(color)]));
	const ink = token("--foreground");
	const muted = token("--muted-foreground");
	const faint = token("--subtle-foreground");
	const surface = token("--subpanel");
	const raised = token("--popover");
	const card = token("--card");
	const lifted = token("--hover-card");
	const edge = token("--hover-card-border");
	const line = token("--border");
	const onSeries = token("--on-series");
	return {
		darkMode: theme === "dark",
		background: surface,
		fontFamily: FONT,
		fontSize: "12px",
		primaryColor: raised,
		primaryBorderColor: edge,
		primaryTextColor: ink,
		secondaryColor: lifted,
		secondaryBorderColor: edge,
		secondaryTextColor: ink,
		tertiaryColor: card,
		tertiaryBorderColor: edge,
		tertiaryTextColor: ink,
		mainBkg: raised,
		nodeBorder: edge,
		nodeTextColor: ink,
		textColor: ink,
		titleColor: ink,
		lineColor: muted,
		defaultLinkColor: muted,
		edgeLabelBackground: surface,
		clusterBkg: card,
		clusterBorder: line,
		noteBkgColor: lifted,
		noteBorderColor: edge,
		noteTextColor: ink,
		actorBkg: raised,
		actorBorder: edge,
		actorTextColor: ink,
		actorLineColor: muted,
		signalColor: muted,
		signalTextColor: ink,
		labelBoxBkgColor: raised,
		labelBoxBorderColor: edge,
		labelTextColor: ink,
		loopTextColor: ink,
		activationBkgColor: lifted,
		activationBorderColor: edge,
		sequenceNumberColor: onSeries,
		...Object.fromEntries(series.map((color, i) => [`pie${i + 1}`, color])),
		pieStrokeColor: surface,
		pieStrokeWidth: "2px",
		pieOuterStrokeColor: edge,
		pieOuterStrokeWidth: "1px",
		pieOpacity: "1",
		pieSectionTextColor: onSeries,
		pieSectionTextSize: "12px",
		pieTitleTextColor: ink,
		pieTitleTextSize: "13px",
		pieLegendTextColor: ink,
		pieLegendTextSize: "12px",
		...each("cScale", (color) => color),
		...each("cScaleLabel", () => onSeries),
		...each("cScaleInv", () => ink),
		...each("fillType", (color) => color),
		...each("git", (color) => color),
		...each("gitBranchLabel", () => onSeries),
		...each("gitInv", () => surface),
		commitLabelColor: ink,
		commitLabelBackground: raised,
		commitLabelFontSize: "11px",
		tagLabelColor: ink,
		tagLabelBackground: raised,
		tagLabelBorder: edge,
		tagLabelFontSize: "11px",
		sectionBkgColor: lifted,
		sectionBkgColor2: card,
		altSectionBkgColor: surface,
		taskBkgColor: series[0],
		taskBorderColor: series[0],
		taskTextColor: onSeries,
		taskTextLightColor: ink,
		taskTextDarkColor: onSeries,
		taskTextOutsideColor: ink,
		activeTaskBkgColor: series[2],
		activeTaskBorderColor: series[2],
		doneTaskBkgColor: muted,
		doneTaskBorderColor: muted,
		critBkgColor: series[6],
		critBorderColor: series[6],
		gridColor: line,
		todayLineColor: series[1],
		sectionTextColor: ink,
		quadrant1Fill: raised,
		quadrant2Fill: card,
		quadrant3Fill: card,
		quadrant4Fill: raised,
		quadrant1TextFill: ink,
		quadrant2TextFill: ink,
		quadrant3TextFill: ink,
		quadrant4TextFill: ink,
		quadrantPointFill: series[0],
		quadrantPointTextFill: ink,
		quadrantXAxisTextFill: muted,
		quadrantYAxisTextFill: muted,
		quadrantInternalBorderStrokeFill: line,
		quadrantExternalBorderStrokeFill: edge,
		quadrantTitleFill: ink,
		xyChart: {
			backgroundColor: surface,
			titleColor: ink,
			xAxisTitleColor: muted,
			xAxisLabelColor: muted,
			xAxisTickColor: edge,
			xAxisLineColor: edge,
			yAxisTitleColor: muted,
			yAxisLabelColor: muted,
			yAxisTickColor: edge,
			yAxisLineColor: edge,
			plotColorPalette: series.join(","),
		},
		relationColor: muted,
		relationLabelBackground: surface,
		relationLabelColor: ink,
		attributeBackgroundColorOdd: card,
		attributeBackgroundColorEven: raised,
		radar: {
			axisColor: faint,
			graticuleColor: edge,
			graticuleOpacity: 0.35,
			curveOpacity: 0.35,
			curveStrokeWidth: 2,
		},
		packet: {
			blockFillColor: raised,
			blockStrokeColor: edge,
			labelColor: ink,
			startByteColor: muted,
			endByteColor: muted,
			titleColor: ink,
		},
		archEdgeColor: muted,
		archEdgeArrowColor: muted,
		archGroupBorderColor: edge,
	};
};

/**
 * What the theme can't reach. A sankey's links multiply with what's under
 * them, which goes near black on a dark panel, and a mindmap's round root
 * starts its label at its middle.
 */
export const DIAGRAM_CSS =
	".link{mix-blend-mode:normal !important}.mindmap-node circle + .label text{text-anchor:middle}";
