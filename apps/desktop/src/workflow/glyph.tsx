// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A node's status as a glyph on its lane, drawn from the shared glyph
// table. Every status has its own shape as well as its color, so it reads
// in grey and with less motion, and each says its status to a screen
// reader.

import type { CSSProperties } from "react";

import type { NodeStatus } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import { type GlyphColor, type GlyphShape, glyphOf } from "@maxhq/shared/workflow/glyphs";

const R = 5;

const draw = (shape: GlyphShape, provisional: boolean) => {
	switch (shape) {
		case "ring":
			return (
				<>
					<circle r={R} fill="none" stroke="currentColor" strokeWidth={1.5} />
					<circle r={2} fill="currentColor" />
				</>
			);
		case "spinner":
			return (
				<>
					<circle r={3} fill="currentColor" />
					<path
						d={`M 0 ${-R} A ${R} ${R} 0 0 1 ${R} 0`}
						fill="none"
						stroke="currentColor"
						strokeWidth={1.6}
						strokeLinecap="round"
						className="animate-busy"
						style={{ transformOrigin: "0 0" }}
					/>
				</>
			);
		case "half":
			return (
				<>
					<circle r={R} fill="none" stroke="currentColor" strokeWidth={1.5} />
					<path d={`M 0 ${-R} A ${R} ${R} 0 0 0 0 ${R} Z`} fill="currentColor" />
				</>
			);
		case "person":
			return (
				<>
					<circle r={R + 0.5} fill="currentColor" />
					<circle r={1.4} cy={-1.6} className="fill-subpanel" />
					<path d="M -2.6 3 A 2.6 2.4 0 0 1 2.6 3 Z" className="fill-subpanel" />
				</>
			);
		case "check":
			return (
				<>
					<circle r={R + 0.5} fill="currentColor" />
					<path
						d="M -2.6 0.2 L -0.8 2 L 2.6 -1.6"
						fill="none"
						strokeWidth={1.6}
						strokeLinecap="round"
						strokeLinejoin="round"
						className="stroke-status-ready-foreground"
					/>
				</>
			);
		case "slash":
			return (
				<>
					<circle
						r={R}
						fill="none"
						stroke="currentColor"
						strokeWidth={1.4}
						strokeDasharray="2 1.6"
					/>
					<path d="M -3 3 L 3 -3" stroke="currentColor" strokeWidth={1.4} />
				</>
			);
		case "ghost":
			return (
				<>
					<circle
						r={R}
						fill="none"
						stroke="currentColor"
						strokeWidth={1.4}
						strokeDasharray="2 1.6"
					/>
					{provisional ? (
						<text
							y={2.6}
							textAnchor="middle"
							fontSize={7.5}
							fontWeight={700}
							fill="currentColor"
						>
							?
						</text>
					) : null}
				</>
			);
		case "cross":
			return (
				<>
					<circle r={R + 0.5} fill="currentColor" />
					<path
						d="M -2.2 -2.2 L 2.2 2.2 M 2.2 -2.2 L -2.2 2.2"
						strokeWidth={1.6}
						strokeLinecap="round"
						className="stroke-subpanel"
					/>
				</>
			);
		case "lock":
			return (
				<>
					<rect x={-4} y={-1} width={8} height={6} rx={1.2} fill="currentColor" />
					<path
						d="M -2.4 -1 V -2.6 A 2.4 2.4 0 0 1 2.4 -2.6 V -1"
						fill="none"
						stroke="currentColor"
						strokeWidth={1.4}
					/>
				</>
			);
		case "striped":
			return (
				<>
					<circle r={R} fill="none" stroke="currentColor" strokeWidth={1.4} />
					<path
						d="M -3.5 1.5 L 1.5 -3.5 M -1.5 3.5 L 3.5 -1.5"
						stroke="currentColor"
						strokeWidth={1.2}
					/>
				</>
			);
		case "unknown":
			return (
				<>
					<circle r={R} fill="none" stroke="currentColor" strokeWidth={1.4} />
					<path d="M -2.4 0 H 2.4" stroke="currentColor" strokeWidth={1.4} />
				</>
			);
		// Omitted nodes don't get a row, and queued ones are a plain ring.
		case "hollow":
		case "faint":
			return <circle r={R} fill="none" stroke="currentColor" strokeWidth={1.5} />;
			return <circle r={R} fill="none" stroke="currentColor" strokeWidth={1.5} />;
	}
};

/** The text color each glyph color takes, for anything drawn in a status's color. */
export const GLYPH_COLOR: Record<GlyphColor, string> = {
	faint: "text-muted-foreground",
	quiet: "text-muted-foreground",
	muted: "text-muted-foreground",
	plain: "text-secondary-foreground",
	accent: "text-primary",
	warn: "text-status-busy",
	good: "text-status-ready",
	bad: "text-status-error",
};

/** A node's status glyph, 12 px, named for screen readers. */
export const NodeGlyph = ({
	status,
	provisional,
	className,
	style,
}: {
	status: NodeStatus;
	provisional: boolean;
	className?: string;
	style?: CSSProperties;
}) => {
	const glyph = glyphOf(status, provisional);
	return (
		<svg
			role="img"
			aria-label={glyph.label}
			width={12}
			height={12}
			viewBox="-6 -6 12 12"
			className={cn(
				"shrink-0 overflow-visible",
				GLYPH_COLOR[glyph.color],
				glyph.dim && "opacity-60",
				glyph.motion === "pulse" && "animate-needs-you",
				className
			)}
			style={style}
		>
			{draw(glyph.shape, provisional)}
		</svg>
	);
};
