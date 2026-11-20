// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A slider that snaps to a few named stops, such as effort levels, as the
// desktop's: each stop's name sits under it, the current one in bold, and
// the stop marked as the default says so. A tap or a drag picks a stop,
// and the phone ticks as the thumb reaches each one; VoiceOver swipes up or
// down to move it. A pick applies at once; a drag shows where it goes and
// picks when it lets go.

import { t, useI18n } from "@maxhq/shared/i18n";

import * as Haptics from "expo-haptics";
import { useEffect, useState } from "react";
import { type DimensionValue, PanResponder, StyleSheet, Text, View } from "react-native";

import {
	type Picked,
	shownValue,
	type Stop,
	stopAt,
	stopCenter,
	stopIndex,
	stopText,
	stopToPick,
} from "@maxhq/shared/ui/step-slider";

import { color, size, tint, TOUCH } from "../theme";

const THUMB = 22;
const DOT = 6;
const LINE = 3;

/** What a drag needs from the slider's latest render. */
type Latest = {
	readonly count: number;
	/** The stop the slider shows. */
	readonly at: number;
	readonly off: boolean;
	readonly onPick: (index: number) => void;
};

// The drag, kept outside React as a swipe row's slide is: the slider's
// width, where the finger went down, the stop under it, and the gesture.
// `onDrag` takes the stop under the finger, or null once it lets go.
const createDrag = (onDrag: (index: number | null) => void) => {
	let width = 0;
	let start = 0;
	let over = 0;
	let latest: Latest = { count: 0, at: 0, off: true, onPick: () => {} };
	const under = (dx: number) => stopAt(start + dx, width, latest.count);
	const reach = (index: number) => {
		if (index !== over) void Haptics.selectionAsync();
		over = index;
		onDrag(index);
	};
	const pan = PanResponder.create({
		onStartShouldSetPanResponder: () => !latest.off && width > 0 && latest.count > 0,
		onPanResponderTerminationRequest: () => false,
		onPanResponderGrant: (event) => {
			start = event.nativeEvent.locationX;
			over = latest.at;
			reach(under(0));
		},
		onPanResponderMove: (_, gesture) => reach(under(gesture.dx)),
		onPanResponderRelease: (_, gesture) => {
			onDrag(null);
			latest.onPick(under(gesture.dx));
		},
		onPanResponderTerminate: () => onDrag(null),
	});
	return {
		pan,
		setWidth: (to: number) => {
			width = to;
		},
		update: (next: Latest) => {
			latest = next;
		},
	};
};

/** A share of the slider's width, as a length. */
const share = (part: number): DimensionValue => `${part * 100}%`;

export const StepSlider = ({
	label,
	stops,
	value,
	onPick,
	disabled = false,
}: {
	/** What the slider sets, for VoiceOver. */
	label: string;
	stops: readonly Stop[];
	value: string;
	onPick: (value: string) => void;
	disabled?: boolean;
}) => {
	useI18n();
	const [picked, setPicked] = useState<Picked | null>(null);
	const [dragging, setDragging] = useState<number | null>(null);
	const [drag] = useState(() => createDrag(setDragging));
	const shown = shownValue(picked, value);
	const count = stops.length;
	const at = dragging ?? stopIndex(stops, shown);
	const current = stops[at];
	const marked = stops.some((stop) => stop.isDefault);

	const pick = (index: number) => {
		const stop = stopToPick(stops, index, shown);
		if (!stop) return;
		setPicked({ from: value, to: stop.value });
		onPick(stop.value);
	};
	useEffect(() => drag.update({ count, at, off: disabled, onPick: pick }));

	if (count === 0) return null;
	const ends = share(stopCenter(0, count));
	return (
		<View
			accessible
			accessibilityRole="adjustable"
			accessibilityLabel={label}
			accessibilityValue={{
				min: 0,
				max: count - 1,
				now: at,
				text: current && stopText(current),
			}}
			accessibilityState={{ disabled }}
			accessibilityActions={[{ name: "increment" }, { name: "decrement" }]}
			onAccessibilityAction={(event) => {
				if (disabled) return;
				if (event.nativeEvent.actionName === "increment") pick(at + 1);
				if (event.nativeEvent.actionName === "decrement") pick(at - 1);
			}}
			onLayout={(event) => drag.setWidth(event.nativeEvent.layout.width)}
			pointerEvents="box-only"
			style={disabled && styles.off}
			{...drag.pan.panHandlers}
		>
			<View style={styles.track}>
				<View style={[styles.line, { left: ends, right: ends }]} />
				<View
					style={[styles.line, styles.fill, { left: ends, width: share(at / count) }]}
				/>
				{stops.map((stop, index) => (
					<View
						key={stop.value}
						style={[
							styles.dot,
							index <= at && styles.dotOn,
							{ left: share(stopCenter(index, count)) },
						]}
					/>
				))}
				<View style={[styles.thumb, { left: share(stopCenter(at, count)) }]} />
			</View>
			<View style={styles.names}>
				{stops.map((stop, index) => (
					<View key={stop.value} style={styles.column}>
						<Text
							numberOfLines={1}
							adjustsFontSizeToFit
							minimumFontScale={0.8}
							style={[styles.name, index === at && styles.nameOn]}
						>
							{stop.label}
						</Text>
						{marked ? (
							<Text style={styles.default}>{stop.isDefault ? t("Default") : ""}</Text>
						) : null}
					</View>
				))}
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	off: { opacity: 0.5 },
	track: { height: TOUCH },
	line: {
		position: "absolute",
		top: (TOUCH - LINE) / 2,
		height: LINE,
		borderRadius: LINE / 2,
		backgroundColor: tint(color.mutedForeground, 0.25),
	},
	fill: { backgroundColor: color.primary },
	dot: {
		position: "absolute",
		top: (TOUCH - DOT) / 2,
		width: DOT,
		height: DOT,
		marginLeft: -DOT / 2,
		borderRadius: DOT / 2,
		backgroundColor: tint(color.mutedForeground, 0.45),
	},
	dotOn: { backgroundColor: color.primary },
	thumb: {
		position: "absolute",
		top: (TOUCH - THUMB) / 2,
		width: THUMB,
		height: THUMB,
		marginLeft: -THUMB / 2,
		borderRadius: THUMB / 2,
		borderWidth: 2,
		borderColor: color.primary,
		backgroundColor: color.background,
	},
	names: { flexDirection: "row", marginTop: -6 },
	column: { flex: 1, alignItems: "center", gap: 1, paddingHorizontal: 2 },
	name: { color: color.mutedForeground, fontSize: size.tiny },
	nameOn: { color: color.foreground, fontWeight: "600" },
	default: { color: color.mutedForeground, fontSize: 10, lineHeight: 12, minHeight: 12 },
});
