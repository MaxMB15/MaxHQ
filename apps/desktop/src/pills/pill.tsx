// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem
// Portions copyright (c) 2026 T3 Tools Inc., see THIRD_PARTY_NOTICES

// Adapted from MIT-licensed code, see THIRD_PARTY_NOTICES: rewritten as arrow
// functions. The kinds become MaxHQ's four families and the look moves to
// pills.css, so the text box's pull request decoration can wear it too. A
// state sets the shade, and an upload fills the pill.

import { mergeProps } from "@base-ui/react/merge-props";
import { useRender } from "@base-ui/react/use-render";
import type { GitItemKind, GitItemState } from "@maxhq/shared/pills/wire";
import {
	CircleCheck,
	CircleDot,
	CircleSlash,
	GitMerge,
	GitPullRequest,
	GitPullRequestClosed,
	GitPullRequestDraft,
	type LucideIcon,
	type LucideProps,
} from "lucide-react";
import { type CSSProperties, type ComponentProps, createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { cn } from "@/lib/utils";

import "./pills.css";

/** The hue: files, quotes, agents, notes, and a pull request's or issue's state. */
export type PillFamily =
	| "file"
	| "quote"
	| "agent"
	| "note"
	| "visual"
	| "git-open"
	| "git-draft"
	| "git-merged"
	| "git-closed";

/** The shade. Staging and uploading fill as they go. */
export type PillState = "ready" | "staging" | "uploading" | "failed" | "unresolved";

type PillProps = useRender.ComponentProps<"span"> & {
	readonly family: PillFamily;
	readonly state?: PillState;
	/** How much has uploaded, from 0 to 1. */
	readonly progress?: number | null;
};

/** One piece of context, as one inline pill. */
export const Pill = ({
	family,
	state = "ready",
	progress,
	className,
	render,
	...props
}: PillProps) => {
	const style: CSSProperties | undefined =
		progress == null
			? undefined
			: ({
					"--pill-progress": `${Math.round(Math.min(1, Math.max(0, progress)) * 100)}%`,
				} as CSSProperties);
	const own = {
		className: cn("pill", className),
		"data-slot": "pill",
		"data-family": family,
		"data-state": state,
		style,
	};
	return useRender({ defaultTagName: "span", props: mergeProps<"span">(own, props), render });
};

/** The pill's words, cut with an ellipsis when the pill runs out of room. */
export const PillLabel = ({ className, ...props }: ComponentProps<"span">) => (
	<span className={cn("pill-label", className)} data-slot="pill-label" {...props} />
);

/** The hue for a pull request's or issue's state, the way GitHub colors it. */
export const gitFamily = (state: GitItemState): PillFamily => {
	switch (state) {
		case "open":
			return "git-open";
		case "merged":
		case "completed":
			return "git-merged";
		case "closed":
			return "git-closed";
		case "draft":
		case "not_planned":
			return "git-draft";
	}
};

/** The icon for a pull request or issue in its state. */
export const gitIcon = (kind: GitItemKind, state: GitItemState): LucideIcon => {
	if (kind === "issue") {
		if (state === "open") return CircleDot;
		if (state === "not_planned") return CircleSlash;
		return CircleCheck;
	}
	switch (state) {
		case "draft":
			return GitPullRequestDraft;
		case "merged":
			return GitMerge;
		case "closed":
			return GitPullRequestClosed;
		default:
			return GitPullRequest;
	}
};

/** A pull request's or issue's icon in its state, as an element. */
export const GitIcon = ({
	kind,
	state,
	...props
}: LucideProps & { readonly kind: GitItemKind; readonly state: GitItemState }) =>
	createElement(gitIcon(kind, state), props);

const masks = new Map<LucideIcon, string>();

/**
 * The icon as a CSS mask, for the text box's pull request decoration,
 * which draws its icon without React.
 */
export const gitIconMask = (kind: GitItemKind, state: GitItemState): string => {
	const icon = gitIcon(kind, state);
	let mask = masks.get(icon);
	if (mask === undefined) {
		const svg = renderToStaticMarkup(
			createElement(icon, { xmlns: "http://www.w3.org/2000/svg" })
		);
		mask = `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
		masks.set(icon, mask);
	}
	return mask;
};
