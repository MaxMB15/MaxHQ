// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A workflow node's status as a glyph, drawn as the desktop's from the
// shared glyph table. Every status has its own shape as well as its color,
// so it reads in grey, and each says its status to a screen reader unless
// the row around it says it already.

import { memo } from "react";
import { StyleSheet, View } from "react-native";
import Svg, { Circle, Path, Rect, Text as SvgText } from "react-native-svg";

import type { NodeStatus } from "@maxhq/shared/bindings";
import { type GlyphShape, glyphOf } from "@maxhq/shared/workflow/glyphs";

import { color } from "../../theme";
import { Spinner } from "../../ui/Dot";
import { GLYPH_TINT } from "./runs";

const R = 5;

const draw = (shape: GlyphShape, provisional: boolean, ink: string, surface: string) => {
	switch (shape) {
		case "ring":
			return (
				<>
					<Circle r={R} fill="none" stroke={ink} strokeWidth={1.5} />
					<Circle r={2} fill={ink} />
				</>
			);
		case "half":
			return (
				<>
					<Circle r={R} fill="none" stroke={ink} strokeWidth={1.5} />
					<Path d={`M 0 ${-R} A ${R} ${R} 0 0 0 0 ${R} Z`} fill={ink} />
				</>
			);
		case "person":
			return (
				<>
					<Circle r={R + 0.5} fill={ink} />
					<Circle r={1.4} cy={-1.6} fill={surface} />
					<Path d="M -2.6 3 A 2.6 2.4 0 0 1 2.6 3 Z" fill={surface} />
				</>
			);
		case "check":
			return (
				<>
					<Circle r={R + 0.5} fill={ink} />
					<Path
						d="M -2.6 0.2 L -0.8 2 L 2.6 -1.6"
						fill="none"
						stroke={color.readyForeground}
						strokeWidth={1.6}
						strokeLinecap="round"
						strokeLinejoin="round"
					/>
				</>
			);
		case "slash":
			return (
				<>
					<Circle
						r={R}
						fill="none"
						stroke={ink}
						strokeWidth={1.4}
						strokeDasharray="2 1.6"
					/>
					<Path d="M -3 3 L 3 -3" stroke={ink} strokeWidth={1.4} />
				</>
			);
		case "ghost":
			return (
				<>
					<Circle
						r={R}
						fill="none"
						stroke={ink}
						strokeWidth={1.4}
						strokeDasharray="2 1.6"
					/>
					{provisional ? (
						<SvgText
							y={2.6}
							textAnchor="middle"
							fontSize={7.5}
							fontWeight="700"
							fill={ink}
						>
							?
						</SvgText>
					) : null}
				</>
			);
		case "cross":
			return (
				<>
					<Circle r={R + 0.5} fill={ink} />
					<Path
						d="M -2.2 -2.2 L 2.2 2.2 M 2.2 -2.2 L -2.2 2.2"
						stroke={surface}
						strokeWidth={1.6}
						strokeLinecap="round"
					/>
				</>
			);
		case "lock":
			return (
				<>
					<Rect x={-4} y={-1} width={8} height={6} rx={1.2} fill={ink} />
					<Path
						d="M -2.4 -1 V -2.6 A 2.4 2.4 0 0 1 2.4 -2.6 V -1"
						fill="none"
						stroke={ink}
						strokeWidth={1.4}
					/>
				</>
			);
		case "striped":
			return (
				<>
					<Circle r={R} fill="none" stroke={ink} strokeWidth={1.4} />
					<Path
						d="M -3.5 1.5 L 1.5 -3.5 M -1.5 3.5 L 3.5 -1.5"
						stroke={ink}
						strokeWidth={1.2}
					/>
				</>
			);
		case "unknown":
			return (
				<>
					<Circle r={R} fill="none" stroke={ink} strokeWidth={1.4} />
					<Path d="M -2.4 0 H 2.4" stroke={ink} strokeWidth={1.4} />
				</>
			);
		// The spinner turns as a view, below.
		case "spinner":
		case "hollow":
		case "faint":
			return <Circle r={R} fill="none" stroke={ink} strokeWidth={1.5} />;
	}
};

/** A node's status glyph, 12 points unless `size` says otherwise. */
export const NodeGlyph = memo(
	({
		status,
		provisional = false,
		size = 12,
		decorative = false,
		surface = color.background,
	}: {
		status: NodeStatus;
		provisional?: boolean;
		size?: number;
		/** Set where the row's own label says the status, so it isn't said twice. */
		decorative?: boolean;
		/** The color behind it, which the person and the cross cut out. */
		surface?: string;
	}) => {
		const glyph = glyphOf(status, provisional);
		const ink = GLYPH_TINT[glyph.color];
		const a11y = decorative
			? {
					accessibilityElementsHidden: true,
					importantForAccessibility: "no-hide-descendants" as const,
				}
			: {
					accessible: true,
					accessibilityRole: "image" as const,
					accessibilityLabel: glyph.label,
				};
		const box = { width: size, height: size, opacity: glyph.dim ? 0.6 : 1 };
		if (glyph.shape === "spinner") {
			const dot = size / 2;
			return (
				<View {...a11y} style={[styles.center, box]}>
					<Spinner tone={ink} size={size} width={1.5} track={0.2} />
					<View
						style={[
							styles.dot,
							{
								width: dot,
								height: dot,
								borderRadius: dot / 2,
								backgroundColor: ink,
							},
						]}
					/>
				</View>
			);
		}
		return (
			<View {...a11y} style={box}>
				<Svg width={size} height={size} viewBox="-6 -6 12 12">
					{draw(glyph.shape, provisional, ink, surface)}
				</Svg>
			</View>
		);
	}
);
NodeGlyph.displayName = "NodeGlyph";

const styles = StyleSheet.create({
	center: { alignItems: "center", justifyContent: "center" },
	dot: { position: "absolute" },
});
