// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A list row that slides left to show its actions: a short swipe snaps it open, a tap on the row closes it. The same
// actions are offered to VoiceOver, which can't swipe, with any the row
// offers only to VoiceOver. A long press can open more. The phone taps
// lightly as the row snaps open.

import * as Haptics from "expo-haptics";
import { type ReactNode, useEffect, useState } from "react";
import { Animated, PanResponder, Pressable, StyleSheet, Text, View } from "react-native";

import { color, size } from "../theme";

/** One action VoiceOver offers on the row, by its label. */
export type SpokenAction = {
	readonly label: string;
	readonly onPress: () => void;
};

/** One button behind the row. */
export type SwipeAction = SpokenAction & {
	readonly tone: "primary" | "secondary";
};

// Each action is 58 wide, and the row opens once it's
// pulled past 42% of them.
const ACTION_WIDTH = 58;
const OPEN_AT = 0.42;
const SPRING = { damping: 26, mass: 0.7, stiffness: 330, overshootClamping: true };
const NONE: readonly SpokenAction[] = [];

// The row's slide, kept outside React: where it is, whether it's open, and
// the gesture that moves it.
const createSlide = () => {
	const x = new Animated.Value(0);
	let open = false;
	let width = 0;
	const snap = (to: number) => {
		if (to !== 0 && !open) void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
		open = to !== 0;
		Animated.spring(x, { toValue: to, useNativeDriver: true, ...SPRING }).start();
	};
	const from = () => (open ? -width : 0);
	const pan = PanResponder.create({
		onMoveShouldSetPanResponder: (_, g) =>
			width > 0 && Math.abs(g.dx) > 10 && Math.abs(g.dx) > Math.abs(g.dy) * 1.5,
		onPanResponderMove: (_, g) =>
			x.setValue(Math.min(0, Math.max(-width * 1.2, from() + g.dx))),
		onPanResponderRelease: (_, g) => snap(from() + g.dx < -width * OPEN_AT ? -width : 0),
		onPanResponderTerminate: () => snap(from()),
	});
	return {
		x,
		pan,
		snap,
		isOpen: () => open,
		setWidth: (to: number) => {
			width = to;
		},
	};
};

export const SwipeRow = ({
	actions,
	spoken = NONE,
	onPress,
	onLongPress,
	label,
	children,
}: {
	actions: readonly SwipeAction[];
	/**
	 * Actions only VoiceOver offers, after the swipe actions, such as one
	 * for a button inside the row, which it can't reach since it reads the
	 * row as one.
	 */
	spoken?: readonly SpokenAction[];
	onPress: () => void;
	onLongPress?: () => void;
	/** What VoiceOver reads for the row. */
	label: string;
	children: ReactNode;
}) => {
	const [slide] = useState(createSlide);
	const width = actions.length * ACTION_WIDTH;
	const offered = [...actions, ...spoken];
	useEffect(() => slide.setWidth(width), [slide, width]);

	const press = () => (slide.isOpen() ? slide.snap(0) : onPress());
	const act = (action: SwipeAction) => {
		slide.snap(0);
		action.onPress();
	};

	return (
		<View style={styles.wrap}>
			{width > 0 ? (
				<View style={[styles.actions, { width }]}>
					{actions.map((action) => (
						<Pressable
							key={action.label}
							accessibilityRole="button"
							onPress={() => act(action)}
							style={[
								styles.action,
								{
									backgroundColor:
										action.tone === "primary" ? color.primary : color.input,
								},
							]}
						>
							<Text
								numberOfLines={1}
								style={[
									styles.actionLabel,
									action.tone === "primary" && { color: color.primaryForeground },
								]}
							>
								{action.label}
							</Text>
						</Pressable>
					))}
				</View>
			) : null}
			<Animated.View
				style={{ transform: [{ translateX: slide.x }] }}
				{...slide.pan.panHandlers}
			>
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={label}
					accessibilityActions={offered.map((a) => ({ name: a.label, label: a.label }))}
					onAccessibilityAction={(event) =>
						offered.find((a) => a.label === event.nativeEvent.actionName)?.onPress()
					}
					onPress={press}
					onLongPress={onLongPress}
					style={({ pressed }) => [styles.row, pressed && styles.pressed]}
				>
					{children}
				</Pressable>
			</Animated.View>
		</View>
	);
};

const styles = StyleSheet.create({
	wrap: { overflow: "hidden", borderRadius: 6, backgroundColor: color.background },
	actions: {
		position: "absolute",
		top: 0,
		bottom: 0,
		right: 0,
		flexDirection: "row",
	},
	action: { flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: 4 },
	actionLabel: { color: color.foreground, fontSize: size.meta, fontWeight: "600" },
	row: { backgroundColor: color.background },
	pressed: { backgroundColor: color.accent },
});
