// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file's diff through @pierre/diffs, highlighted in a pool of two workers.
// It loads on its own, the first time you open a file change. The portable
// worker carries everything it needs in one file, so it builds as a plain
// script and needs no WebAssembly.

import { PatchDiff, WorkerPoolContextProvider } from "@pierre/diffs/react";
import DiffsWorker from "@pierre/diffs/worker/worker-portable.js?worker";
import { type CSSProperties, useMemo } from "react";

import { useTheme } from "@/app/theme";

const THEME = { dark: "github-dark-default", light: "github-light-default" };

const POOL = { workerFactory: () => new DiffsWorker(), poolSize: 2 };

const HIGHLIGHTER = {
	theme: THEME,
	preferredHighlighter: "shiki-js" as const,
	tokenizeMaxLineLength: 1000,
};

const OPTIONS = {
	theme: THEME,
	preferredHighlighter: "shiki-js" as const,
	diffStyle: "unified" as const,
	diffIndicators: "classic" as const,
	hunkSeparators: "simple" as const,
	overflow: "scroll" as const,
	disableFileHeader: true,
	tokenizeMaxLineLength: 1000,
};

// The app's colors, handed into the diff's shadow root.
const STYLE = {
	"--diffs-dark-bg": "var(--subpanel)",
	"--diffs-dark": "var(--terminal)",
	"--diffs-light-bg": "var(--subpanel)",
	"--diffs-light": "var(--terminal)",
	"--diffs-font-family": "var(--mono-stack)",
	"--diffs-font-size": "11.5px",
	"--diffs-line-height": "19px",
	"--diffs-bg-context-override": "var(--subpanel)",
	"--diffs-bg-separator-override": "var(--subpanel)",
	"--diffs-fg-number-override": "var(--subtle-foreground)",
	"--diffs-bg-addition-override": "color-mix(in srgb, var(--status-ready) 8%, transparent)",
	"--diffs-bg-deletion-override": "color-mix(in srgb, var(--destructive) 8%, transparent)",
	"--diffs-addition-color-override": "var(--diff-added)",
	"--diffs-deletion-color-override": "var(--diff-removed)",
} as CSSProperties;

/** One file's unified diff, highlighted in the app's theme. */
const PierreDiff = ({ patch }: { patch: string }) => {
	const theme = useTheme();
	const options = useMemo(() => ({ ...OPTIONS, themeType: theme }), [theme]);
	return (
		<WorkerPoolContextProvider poolOptions={POOL} highlighterOptions={HIGHLIGHTER}>
			<PatchDiff patch={patch} options={options} style={STYLE} />
		</WorkerPoolContextProvider>
	);
};

export default PierreDiff;
