// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The desktop app's dark colors, from its globals.css, under the same names,
// and the type the phone's screens use. The phone is dark only.

import type { CodeColor } from "@maxhq/shared/agent-view/highlight";

import { platform } from "./platform";

export const color = {
	background: "#0f1217",
	foreground: "#e8ecf2",
	foregroundStrong: "#f3f6fa",
	card: "#14181f",
	secondary: "#1a1f27",
	secondaryForeground: "#b8c0cc",
	mutedForeground: "#8b94a3",
	subtleForeground: "#5b6472",
	accent: "#1c2129",
	primary: "#4692f6",
	primaryForeground: "#071225",
	destructive: "#f0616d",
	border: "#222832",
	input: "#2c333e",
	track: "#262c36",
	subpanel: "#12161c",
	terminal: "#c9d0da",
	canvas: "#0b0e12",
	rowDivider: "#1c2129",
	busy: "#e8c547",
	ready: "#3fb37f",
	readyForeground: "#06140d",
	warm: "#8fe3b5",
	linked: "#a07cf0",
	/** A visual's diamond, the desktop's fifth chart color. */
	visual: "#d55181",
	connecting: "#f08a3c",
	error: "#f0616d",
	off: "#5b6472",
	readyText: "#9fd9bc",
	busyText: "#e9d9a0",
	connectingText: "#f5b88a",
	errorText: "#f4a3aa",
	findActiveBg: "#eea52b",
} as const;

/** Highlighted code's colors, the desktop's dark `--code-*` ones. */
export const codeColor: Readonly<Record<CodeColor, string>> = {
	text: "#e6edf3",
	comment: "#8b949e",
	keyword: "#ff7b72",
	constant: "#79c0ff",
	variable: "#ffa657",
	function: "#d2a8ff",
	tag: "#7ee787",
	string: "#a5d6ff",
	invalid: "#ffa198",
};

/** A `#rrggbb` color at an opacity from 0 to 1, as the desktop's tinted fills. */
export const tint = (hex: string, alpha: number): string => {
	const n = Number.parseInt(hex.slice(1), 16);
	return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
};

// The desktop names Instrument Sans and JetBrains Mono but bundles neither
// yet, so it shows the system font and Menlo, as the phone does on iOS.
// Android has no Menlo and takes its own monospace. Undefined is the system
// font.
export const font = { sans: undefined, mono: platform.monoFont } as const;

/** Text sizes, as the desktop's compact panes. */
export const size = {
	title: 20,
	heading: 15,
	body: 14,
	small: 13,
	meta: 12,
	tiny: 11.5,
	pill: 11,
} as const;

/** Corner radii: buttons and fields, cards, and big panels. */
export const radius = { sm: 6, md: 8, lg: 11 } as const;

/** The least a control is tall or wide, so a finger finds it. */
export const TOUCH = 44;
