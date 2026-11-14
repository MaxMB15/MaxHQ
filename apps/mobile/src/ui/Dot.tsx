// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The activity and connection dots, drawn with views in the shapes the
// shared package names for each state, as the desktop draws them. Every
// state has its own shape, so none relies on color alone. A dot that sits
// on a card is told the card's color, which its badges and glyphs cut out
// with.

import { useEffect, useMemo } from "react";
import { Animated, Easing, StyleSheet, useAnimatedValue, View, type ViewStyle } from "react-native";

import { ACTIVITY_LABEL, type ActivityLook } from "@maxhq/shared/status/activity";
import { CONNECTION_LABEL, type ConnectionLook } from "@maxhq/shared/status/connection";
import {
	ACTIVITY_DOT,
	CONNECTION_DOT,
	type DotGlyph,
	type DotShape,
	type DotTone,
} from "@maxhq/shared/status/dot";

import { color, tint } from "../theme";

const TONE: Record<DotTone, string> = {
	busy: color.busy,
	ready: color.ready,
	warm: color.warm,
	linked: color.linked,
	connecting: color.connecting,
	error: color.error,
	off: color.off,
	muted: color.mutedForeground,
};

/**
 * A ring turning with a bright arc on a faint track, at the desktop's speeds.
 * Each keeps its own loop, as the desktop's CSS does. A value shared between
 * spinners stops for good once the last of them leaves the screen, which
 * froze every spinner after it.
 */
export const Spinner = ({
	tone = color.busy,
	size = 12,
	width = 2,
	track = 0.25,
	slow = false,
}: {
	tone?: string;
	size?: number;
	width?: number;
	/** How strong the track is against the arc. */
	track?: number;
	/** Turns slower, for a thing starting up. */
	slow?: boolean;
}) => {
	const turn = useAnimatedValue(0);
	const duration = slow ? 1400 : 900;
	useEffect(() => {
		// A native loop runs from where the value stands, so each turn starts at the top.
		turn.setValue(0);
		const loop = Animated.loop(
			Animated.timing(turn, {
				toValue: 1,
				duration,
				easing: Easing.linear,
				useNativeDriver: true,
			})
		);
		loop.start();
		return () => loop.stop();
	}, [turn, duration]);
	const rotate = useMemo(
		() => turn.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] }),
		[turn]
	);
	return (
		<Animated.View
			style={{
				width: size,
				height: size,
				borderRadius: size / 2,
				borderWidth: width,
				borderColor: tint(tone, track),
				borderTopColor: tone,
				transform: [{ rotate }],
			}}
		/>
	);
};

/** A bar of a glyph: where its middle sits, how long it is, and its turn. */
type Bar = {
	readonly x: number;
	readonly y: number;
	readonly length: number;
	readonly deg: number;
};

// Glyphs on a 10 point dot, as the desktop's: a clock's hands, a pause's
// two bars, a cross.
const GLYPH: Record<DotGlyph, readonly Bar[]> = {
	clock: [
		{ x: 5, y: 3.7, length: 3, deg: 0 },
		{ x: 5.8, y: 5.5, length: 2.2, deg: -60 },
	],
	pause: [
		{ x: 3.8, y: 5, length: 4.2, deg: 0 },
		{ x: 6.2, y: 5, length: 4.2, deg: 0 },
	],
	cross: [
		{ x: 5, y: 5, length: 5, deg: 45 },
		{ x: 5, y: 5, length: 5, deg: -45 },
	],
};

const Glyph = ({ glyph, size, surface }: { glyph: DotGlyph; size: number; surface: string }) => {
	const scale = size / 10;
	const width = 1.3 * scale;
	return GLYPH[glyph].map((bar, i) => (
		<View
			key={i}
			style={{
				position: "absolute",
				left: bar.x * scale - width / 2,
				top: (bar.y - bar.length / 2) * scale,
				width,
				height: bar.length * scale,
				borderRadius: width / 2,
				backgroundColor: surface,
				transform: [{ rotate: `${bar.deg}deg` }],
			}}
		/>
	));
};

const Mark = ({
	shape,
	size,
	surface,
	label,
	track,
}: {
	shape: DotShape;
	size: number;
	surface: string;
	label: string | undefined;
	/** How strong a spinner's track is, as on the desktop's dot of its kind. */
	track: number;
}) => {
	const tone = TONE[shape.tone];
	const ring = size >= 10 ? 1.5 : 1.25;
	const box: ViewStyle = { width: size, height: size, borderRadius: size / 2 };
	const center = Math.max(3, Math.round(size * 0.4));
	const inner = (fill: string) => (
		<View
			style={{
				width: center,
				height: center,
				borderRadius: center / 2,
				backgroundColor: fill,
			}}
		/>
	);
	const a11y = label
		? { accessible: true, accessibilityRole: "image" as const, accessibilityLabel: label }
		: {
				accessibilityElementsHidden: true,
				importantForAccessibility: "no-hide-descendants" as const,
			};

	const body = () => {
		switch (shape.kind) {
			case "fill":
				return <View style={[box, { backgroundColor: tone }]} />;
			case "spin":
				return (
					<Spinner
						tone={tone}
						size={size}
						width={size >= 10 ? 2 : 1.5}
						track={track}
						slow={shape.slow}
					/>
				);
			case "badge": {
				const badge = Math.max(4, size * 0.45);
				return (
					<View style={[box, { backgroundColor: tone }]}>
						<View
							style={{
								position: "absolute",
								top: -2,
								right: -2,
								width: badge,
								height: badge,
								borderRadius: badge / 2,
								borderWidth: 1,
								borderColor: surface,
								backgroundColor: color.foregroundStrong,
							}}
						/>
					</View>
				);
			}
			case "ring":
				return (
					<View
						style={[
							box,
							{
								borderWidth: ring,
								borderColor: tone,
								borderStyle: shape.dashed ? "dashed" : "solid",
							},
						]}
					/>
				);
			case "ring_dot":
				return (
					<View style={[box, styles.center, { borderWidth: 2, borderColor: tone }]}>
						{inner(TONE[shape.dot])}
					</View>
				);
			case "halo":
				return (
					<View style={[box, styles.center, { borderWidth: ring, borderColor: tone }]}>
						{inner(tone)}
					</View>
				);
			case "orbit":
				return (
					<View
						style={[
							box,
							styles.center,
							{ borderWidth: ring, borderColor: tone, borderStyle: "dashed" },
						]}
					>
						{inner(TONE[shape.dot])}
					</View>
				);
			case "glyph":
				return (
					<View style={[box, { backgroundColor: tone }]}>
						<Glyph glyph={shape.glyph} size={size} surface={surface} />
					</View>
				);
			case "half":
				return (
					<View style={[box, styles.clip, { borderWidth: ring, borderColor: tone }]}>
						<View style={[styles.half, { backgroundColor: tone }]} />
					</View>
				);
			case "slash":
				return (
					<View style={[box, styles.clip, { borderWidth: ring, borderColor: tone }]}>
						<View
							style={{
								position: "absolute",
								left: (size - 3 * ring) / 2,
								top: -2,
								width: ring,
								height: size + 4,
								backgroundColor: tone,
								transform: [{ rotate: "45deg" }],
							}}
						/>
					</View>
				);
		}
	};

	return <View {...a11y}>{body()}</View>;
};

type DotProps = {
	/** How wide it is: 10 for an activity dot, 6 to 10 for a connection dot. */
	readonly size?: number;
	/** The color behind it, which its badge and glyph cut out with. */
	readonly surface?: string;
	/** Set when text beside it already says the state. */
	readonly decorative?: boolean;
};

/** The activity dot. */
export const ActivityDot = ({
	look,
	size = 10,
	surface = color.background,
	decorative,
}: DotProps & { look: ActivityLook }) => (
	<Mark
		shape={ACTIVITY_DOT[look]}
		size={size}
		surface={surface}
		label={decorative ? undefined : ACTIVITY_LABEL[look]}
		track={0.25}
	/>
);

/** The connection dot. */
export const ConnectionDot = ({
	look,
	size = 8,
	surface = color.background,
	decorative,
}: DotProps & { look: ConnectionLook }) => (
	<Mark
		shape={CONNECTION_DOT[look]}
		size={size}
		surface={surface}
		label={decorative ? undefined : CONNECTION_LABEL[look]}
		track={0.28}
	/>
);

const styles = StyleSheet.create({
	center: { alignItems: "center", justifyContent: "center" },
	clip: { overflow: "hidden" },
	half: { position: "absolute", left: 0, top: 0, bottom: 0, width: "50%" },
});
