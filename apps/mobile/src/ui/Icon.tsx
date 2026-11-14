// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The few icons the phone shows, drawn with views since it bundles no
// vector library: chevrons, close, more, a gear, a plus, a magnifying
// glass, and the corner marks of a scan frame.

import { View } from "react-native";

import { color } from "../theme";

type IconProps = { readonly size?: number; readonly tint?: string };

const hidden = {
	accessibilityElementsHidden: true,
	importantForAccessibility: "no-hide-descendants" as const,
};

const TURN = { down: 45, right: -45, left: 135, up: -135 } as const;

/** A chevron pointing down, right, left or up. */
export const Chevron = ({
	size = 12,
	tint = color.mutedForeground,
	dir = "down",
	weight = 2,
}: IconProps & { dir?: keyof typeof TURN; weight?: number }) => {
	const arm = size * 0.5;
	// Each arm's corner sits off the middle, so the point lands centered.
	const shift = arm * 0.25;
	const offset = {
		down: { x: 0, y: -shift },
		up: { x: 0, y: shift },
		right: { x: -shift, y: 0 },
		left: { x: shift, y: 0 },
	}[dir];
	return (
		<View
			{...hidden}
			style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
		>
			<View
				style={{
					width: arm,
					height: arm,
					borderRightWidth: weight,
					borderBottomWidth: weight,
					borderColor: tint,
					transform: [
						{ translateX: offset.x },
						{ translateY: offset.y },
						{ rotate: `${TURN[dir]}deg` },
					],
				}}
			/>
		</View>
	);
};

const Bars = ({
	size,
	tint,
	turns,
	length,
	weight,
}: {
	size: number;
	tint: string;
	turns: readonly number[];
	length: number;
	weight: number;
}) => (
	<View
		{...hidden}
		style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
	>
		{turns.map((deg) => (
			<View
				key={deg}
				style={{
					position: "absolute",
					width: weight,
					height: length,
					borderRadius: weight / 2,
					backgroundColor: tint,
					transform: [{ rotate: `${deg}deg` }],
				}}
			/>
		))}
	</View>
);

/** An X. */
export const Close = ({ size = 18, tint = color.secondaryForeground }: IconProps) => (
	<Bars size={size} tint={tint} turns={[45, -45]} length={size * 0.85} weight={1.8} />
);

/** A plus. */
export const Plus = ({ size = 14, tint = color.foreground }: IconProps) => (
	<Bars size={size} tint={tint} turns={[0, 90]} length={size * 0.8} weight={1.8} />
);

/** A magnifying glass: a ring with its handle down to the right. */
export const Search = ({ size = 18, tint = color.mutedForeground }: IconProps) => {
	const ring = size * 0.68;
	const weight = 1.8;
	const handle = size * 0.4;
	// The handle starts on the ring's edge and runs out at 45 degrees.
	const middle = (ring / 2) * (1 + Math.SQRT1_2) + (handle / 2) * Math.SQRT1_2;
	return (
		<View {...hidden} style={{ width: size, height: size }}>
			<View
				style={{
					position: "absolute",
					top: 0,
					left: 0,
					width: ring,
					height: ring,
					borderRadius: ring / 2,
					borderWidth: weight,
					borderColor: tint,
				}}
			/>
			<View
				style={{
					position: "absolute",
					top: middle - handle / 2,
					left: middle - weight / 2,
					width: weight,
					height: handle,
					borderRadius: weight / 2,
					backgroundColor: tint,
					transform: [{ rotate: "-45deg" }],
				}}
			/>
		</View>
	);
};

/** Three dots in a row. */
export const More = ({ size = 18, tint = color.mutedForeground }: IconProps) => (
	<View
		{...hidden}
		style={{
			width: size,
			height: size,
			flexDirection: "row",
			alignItems: "center",
			justifyContent: "center",
			gap: size * 0.17,
		}}
	>
		{[0, 1, 2].map((i) => (
			<View
				key={i}
				style={{ width: 3.4, height: 3.4, borderRadius: 1.7, backgroundColor: tint }}
			/>
		))}
	</View>
);

/** A gear: four bars for its teeth under a ring with a hole. */
export const Gear = ({
	size = 18,
	tint = color.mutedForeground,
	surface = color.background,
}: IconProps & { surface?: string }) => (
	<View
		{...hidden}
		style={{ width: size, height: size, alignItems: "center", justifyContent: "center" }}
	>
		<Bars
			size={size}
			tint={tint}
			turns={[0, 45, 90, 135]}
			length={size * 0.92}
			weight={size * 0.2}
		/>
		<View
			style={{
				position: "absolute",
				width: size * 0.68,
				height: size * 0.68,
				borderRadius: size * 0.34,
				backgroundColor: tint,
				alignItems: "center",
				justifyContent: "center",
			}}
		>
			<View
				style={{
					width: size * 0.3,
					height: size * 0.3,
					borderRadius: size * 0.15,
					backgroundColor: surface,
				}}
			/>
		</View>
	</View>
);

/** The four corner marks of a scan frame. */
export const Corners = ({
	size = 18,
	tint = color.foreground,
	length = size * 0.32,
	weight = 1.8,
}: IconProps & { length?: number; weight?: number }) => {
	const corner = (top: boolean, left: boolean) => ({
		position: "absolute" as const,
		width: length,
		height: length,
		borderColor: tint,
		[top ? "top" : "bottom"]: 0,
		[left ? "left" : "right"]: 0,
		[top ? "borderTopWidth" : "borderBottomWidth"]: weight,
		[left ? "borderLeftWidth" : "borderRightWidth"]: weight,
		[`border${top ? "Top" : "Bottom"}${left ? "Left" : "Right"}Radius`]: weight * 1.5,
	});
	return (
		<View {...hidden} style={{ width: size, height: size }}>
			<View style={corner(true, true)} />
			<View style={corner(true, false)} />
			<View style={corner(false, true)} />
			<View style={corner(false, false)} />
		</View>
	);
};
