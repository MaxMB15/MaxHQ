// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file change's diff. A plain version shows at once, from the patch text
// alone, and @pierre/diffs takes over once its code has loaded. If that
// fails, such as in a test with no workers, the plain version stays.

import { Component, lazy, type ReactNode, Suspense } from "react";

import { patchLines } from "@maxhq/shared/timeline/diff";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const PierreDiff = lazy(() => {
	useI18n();
	return import("./pierre-diff");
});

/** One line of a unified diff, as the plain view shows it. */
export type DiffLine = {
	readonly type: "add" | "remove" | "context" | "gap";
	readonly number: number | null;
	readonly text: string;
};

/**
 * Reads a unified diff into its lines, leaving out the file headers. Only
 * lines under a hunk header show, with a gap between hunks.
 */
export const diffLines = (patch: string): DiffLine[] => {
	const lines: DiffLine[] = [];
	let inHunk = false;
	for (const line of patchLines(patch)) {
		if (line.type === "hunk") {
			if (lines.length > 0) lines.push({ type: "gap", number: null, text: "" });
			inHunk = true;
		} else if (inHunk) lines.push({ ...line, type: line.type });
	}
	return lines;
};

const PLAIN_LIMIT = 400;

const LINE_STYLE: Record<DiffLine["type"], string> = {
	add: "bg-status-ready/8 text-status-ready-text",
	remove: "bg-destructive/8 text-status-error-text",
	context: "text-secondary-foreground",
	gap: "text-subtle-foreground",
};

const SIGN: Record<DiffLine["type"], ReactNode> = {
	add: <span className="text-status-ready">+</span>,
	remove: <span className="text-destructive">−</span>,
	context: null,
	gap: null,
};

/** The diff without highlighting, drawn from the patch text. */
export const PlainDiff = ({ patch }: { patch: string }) => {
	useI18n();
	const lines = diffLines(patch);
	const shown = lines.slice(0, PLAIN_LIMIT);
	return (
		<div className="overflow-x-auto py-1 font-mono text-[11.5px] leading-[19px] select-text">
			{shown.map((line, i) => (
				// Lines have no identity but their place in the patch.
				<div key={i} className={`flex min-w-max ${LINE_STYLE[line.type]}`}>
					<span className="w-[34px] shrink-0 pr-2 text-right text-subtle-foreground tabular-nums select-none">
						{line.type === "gap" ? "⋯" : line.number}
					</span>
					<span className="w-4 shrink-0 select-none">{SIGN[line.type]}</span>
					<span className="pr-3 whitespace-pre">{line.text}</span>
				</div>
			))}
			{lines.length > shown.length ? (
				<div className="px-3 pt-1 text-subtle-foreground">
					{lines.length - shown.length} {translate(" more lines")}
				</div>
			) : null}
		</div>
	);
};

class Fallback extends Component<
	{ fallback: ReactNode; children: ReactNode },
	{ failed: boolean }
> {
	state = { failed: false };

	static getDerivedStateFromError() {
		return { failed: true };
	}

	componentDidCatch(error: unknown) {
		console.warn(translate("Couldn't show the highlighted diff"), error);
	}

	render() {
		return this.state.failed ? this.props.fallback : this.props.children;
	}
}

/** A file change's diff, highlighted once the diff code loads. */
export const DiffView = ({
	patch,
	highlighted = true,
}: {
	patch: string;
	highlighted?: boolean;
}) => {
	useI18n();
	const plain = <PlainDiff patch={patch} />;
	if (!highlighted || typeof Worker === "undefined") return plain;
	return (
		<Fallback fallback={plain}>
			<Suspense fallback={plain}>
				<PierreDiff patch={patch} />
			</Suspense>
		</Fallback>
	);
};
