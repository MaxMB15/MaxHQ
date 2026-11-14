// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The two dots from decisions N-1 and N-2, drawn as the Indicators mockup
// draws them, in the shapes the shared package names for each state. Every
// state has its own shape, so none relies on color alone. A container sets
// `--dot-surface` to its background, which the rings and badges cut out with.

import type { CSSProperties, ReactNode } from "react";

import { type ActivityLook, ACTIVITY_LABEL } from "@maxhq/shared/status/activity";
import { type ConnectionLook, CONNECTION_LABEL } from "@maxhq/shared/status/connection";
import {
	ACTIVITY_DOT,
	CONNECTION_DOT,
	type DotGlyph,
	type DotShape,
	type DotTone,
} from "@maxhq/shared/status/dot";
import { cn } from "@/lib/utils";

/** Whether the dot is announced on its own or sits beside text that says the same. */
type Labelled = {
	/** Set when nearby text already names the state, such as a hover card title. */
	readonly decorative?: boolean;
	/** What a screen reader hears instead of the state's own name. */
	readonly label?: string;
	readonly className?: string;
};

const a11y = (label: string, decorative?: boolean) =>
	decorative ? { "aria-hidden": true } : { role: "img", "aria-label": label };

const TONE: Record<DotTone, string> = {
	busy: "var(--status-busy)",
	ready: "var(--status-ready)",
	warm: "var(--status-warm)",
	linked: "var(--status-linked)",
	connecting: "var(--status-connecting)",
	error: "var(--status-error)",
	off: "var(--status-off)",
	muted: "var(--muted-foreground)",
};

const faint = (tone: DotTone, percent: number) =>
	`color-mix(in srgb, ${TONE[tone]} ${percent}%, transparent)`;

const SURFACE = "var(--dot-surface, var(--sidebar))";

const GLYPH: Record<DotGlyph, { path: string; size: number; width: number }> = {
	clock: { path: "M12 6v6l4 2", size: 8, width: 3.4 },
	pause: { path: "M9 6v12M15 6v12", size: 7, width: 4 },
	cross: { path: "M7 7l10 10M17 7L7 17", size: 7, width: 4.4 },
};

const Glyph = ({ glyph }: { glyph: DotGlyph }) => {
	const { path, size, width } = GLYPH[glyph];
	return (
		<svg
			aria-hidden
			width={size}
			height={size}
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			strokeWidth={width}
			strokeLinecap="round"
		>
			<path d={path} />
		</svg>
	);
};

/** How the two dots differ: their box, size, and how thick a spinner turns. */
type Frame = {
	readonly base: string;
	readonly px: number;
	readonly spinWidth: string;
	readonly trackPercent: number;
};

/** The look's shape, with what the two dots share. */
const Mark = ({
	shape,
	frame,
	look,
	label,
}: {
	shape: DotShape;
	frame: Frame;
	look: string;
	label: ReturnType<typeof a11y>;
}) => {
	const tone = TONE[shape.tone];
	const dot = (extra: string, style?: CSSProperties, children?: ReactNode) => (
		<span data-look={look} className={cn(frame.base, extra)} style={style} {...label}>
			{children}
		</span>
	);
	switch (shape.kind) {
		case "fill":
			return dot("", { backgroundColor: tone });
		case "spin":
			return dot(
				"",
				undefined,
				<span
					className={cn(
						"absolute inset-0 rounded-full",
						frame.spinWidth,
						shape.slow ? "animate-starting" : "animate-busy"
					)}
					style={{
						borderColor: faint(shape.tone, frame.trackPercent),
						borderTopColor: tone,
					}}
				/>
			);
		case "badge":
			return dot(
				"animate-needs-you",
				{ backgroundColor: tone },
				<span className="absolute -top-1 -right-[5px] size-[7px] rounded-full border-[1.5px] border-(--dot-surface,var(--sidebar)) bg-foreground-strong" />
			);
		case "ring":
			return dot(cn("border-[1.5px]", shape.dashed && "border-dashed"), {
				borderColor: tone,
			});
		case "ring_dot":
			return dot(
				"border-2",
				{ borderColor: tone },
				<span
					className="size-1 rounded-full"
					style={{ backgroundColor: TONE[shape.dot] }}
				/>
			);
		case "halo":
			return dot("scale-[0.78]", {
				backgroundColor: tone,
				boxShadow: `0 0 0 2px ${SURFACE}, 0 0 0 3.5px ${tone}`,
			});
		case "orbit":
			return dot(
				"",
				undefined,
				<>
					<span
						className="absolute inset-0 animate-orbit rounded-full border-[1.5px] border-dashed"
						style={{ borderColor: tone }}
					/>
					<span
						className="size-1 rounded-full"
						style={{ backgroundColor: TONE[shape.dot] }}
					/>
				</>
			);
		case "glyph":
			return dot("text-background", { backgroundColor: tone }, <Glyph glyph={shape.glyph} />);
		case "half":
			return dot("border-[1.5px]", {
				borderColor: tone,
				backgroundImage: `linear-gradient(90deg, ${tone} 50%, transparent 50%)`,
			});
		case "slash":
			return dot(
				"overflow-hidden border-[1.5px]",
				{ borderColor: tone },
				<span
					className="absolute top-[-2px] left-1/2 -ml-[0.75px] w-[1.5px] rotate-45"
					style={{ height: frame.px + 4, backgroundColor: tone }}
				/>
			);
	}
};

/** The activity dot, 10 px across. */
export const ActivityDot = ({
	look,
	decorative,
	label: named,
	className,
}: Labelled & { look: ActivityLook }) => (
	<Mark
		shape={ACTIVITY_DOT[look]}
		look={look}
		label={a11y(named ?? ACTIVITY_LABEL[look], decorative)}
		frame={{
			base: cn(
				"relative inline-grid size-2.5 shrink-0 place-items-center rounded-full",
				className
			),
			px: 10,
			spinWidth: "border-2",
			trackPercent: 25,
		}}
	/>
);

/** How big a connection dot is: 6 px in agent rows, 8 px for workspaces, 10 px for devices. */
export type ConnectionDotSize = 6 | 8 | 10;

const SIZE: Record<ConnectionDotSize, string> = { 6: "size-1.5", 8: "size-2", 10: "size-2.5" };

/** The connection dot. */
export const ConnectionDot = ({
	look,
	size = 6,
	decorative,
	label: named,
	className,
}: Labelled & { look: ConnectionLook; size?: ConnectionDotSize }) => (
	<Mark
		shape={CONNECTION_DOT[look]}
		look={look}
		label={a11y(named ?? CONNECTION_LABEL[look], decorative)}
		frame={{
			base: cn("relative inline-block shrink-0 rounded-full", SIZE[size], className),
			px: size,
			spinWidth: size === 10 ? "border-2" : "border-[1.5px]",
			trackPercent: 28,
		}}
	/>
);
