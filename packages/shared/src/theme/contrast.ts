// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The contrast between two CSS colours, as WCAG 2 measures it, for the tests
// that hold each theme's text and controls readable. It reads the forms the
// themes write colours in: hex, `rgb()`, `oklch()`, `color-mix()` in sRGB,
// and `transparent`, `white` and `black`.

/** A colour in sRGB, each channel and its alpha from 0 to 1. */
export type Rgba = {
	readonly r: number;
	readonly g: number;
	readonly b: number;
	readonly a: number;
};

const clamp = (value: number) => Math.min(1, Math.max(0, value));

// A number, or a percentage of `whole`.
const amount = (part: string, whole: number): number =>
	part.endsWith("%") ? (Number.parseFloat(part) / 100) * whole : Number.parseFloat(part);

// What sits inside a function's brackets, split at its own commas.
const argumentsOf = (text: string): string[] => {
	const inner = text.slice(text.indexOf("(") + 1, text.lastIndexOf(")"));
	const parts: string[] = [];
	let depth = 0;
	let start = 0;
	for (let i = 0; i < inner.length; i++) {
		if (inner[i] === "(") depth++;
		else if (inner[i] === ")") depth--;
		else if (inner[i] === "," && depth === 0) {
			parts.push(inner.slice(start, i).trim());
			start = i + 1;
		}
	}
	parts.push(inner.slice(start).trim());
	return parts;
};

// The channels and the alpha after a slash, in either syntax.
const channelsOf = (text: string): { channels: string[]; alpha: string | undefined } => {
	const inner = text.slice(text.indexOf("(") + 1, text.lastIndexOf(")"));
	const [main = "", alpha] = inner.split("/").map((part) => part.trim());
	const channels = main.split(/[\s,]+/).filter(Boolean);
	return { channels, alpha: alpha ?? channels[3] };
};

const hex = (text: string): Rgba | null => {
	const digits = text.slice(1);
	const full = digits.length <= 4 ? [...digits].map((digit) => digit + digit).join("") : digits;
	if (!/^[0-9a-f]{6}([0-9a-f]{2})?$/i.test(full)) return null;
	const byte = (at: number) => Number.parseInt(full.slice(at, at + 2), 16) / 255;
	return { r: byte(0), g: byte(2), b: byte(4), a: full.length === 8 ? byte(6) : 1 };
};

const toGamma = (linear: number) =>
	linear <= 0.0031308 ? 12.92 * linear : 1.055 * linear ** (1 / 2.4) - 0.055;

const toLinear = (gamma: number) =>
	gamma <= 0.04045 ? gamma / 12.92 : ((gamma + 0.055) / 1.055) ** 2.4;

// OKLCH to sRGB, through OKLab and linear light, clipped to the gamut.
const oklch = (lightness: number, chroma: number, hue: number, a: number): Rgba => {
	const radians = (hue * Math.PI) / 180;
	const labA = chroma * Math.cos(radians);
	const labB = chroma * Math.sin(radians);
	const l = (lightness + 0.3963377774 * labA + 0.2158037573 * labB) ** 3;
	const m = (lightness - 0.1055613458 * labA - 0.0638541728 * labB) ** 3;
	const s = (lightness - 0.0894841775 * labA - 1.291485548 * labB) ** 3;
	return {
		r: clamp(toGamma(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s)),
		g: clamp(toGamma(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s)),
		b: clamp(toGamma(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s)),
		a,
	};
};

const NAMED: Record<string, Rgba> = {
	transparent: { r: 0, g: 0, b: 0, a: 0 },
	white: { r: 1, g: 1, b: 1, a: 1 },
	black: { r: 0, g: 0, b: 0, a: 1 },
};

/** A CSS colour, or null for a form this doesn't read. */
export const parseColour = (text: string): Rgba | null => {
	const value = text.trim().toLowerCase();
	if (value in NAMED) return NAMED[value]!;
	if (value.startsWith("#")) return hex(value);
	if (value.startsWith("rgb")) {
		const { channels, alpha } = channelsOf(value);
		const [r = "", g = "", b = ""] = channels;
		return {
			r: amount(r, 255) / 255,
			g: amount(g, 255) / 255,
			b: amount(b, 255) / 255,
			a: alpha === undefined ? 1 : amount(alpha, 1),
		};
	}
	if (value.startsWith("oklch")) {
		const { channels, alpha } = channelsOf(value);
		const [l = "", c = "", h = ""] = channels;
		return oklch(
			amount(l, 1),
			amount(c, 0.4),
			Number.parseFloat(h) || 0,
			alpha ? amount(alpha, 1) : 1
		);
	}
	if (value.startsWith("color-mix(")) {
		const [space, first = "", second = ""] = argumentsOf(value);
		if (space !== "in srgb") return null;
		const split = (part: string) => {
			const match = /^(.*?)\s+([\d.]+%)$/.exec(part);
			return match
				? { colour: match[1]!, share: amount(match[2]!, 1) }
				: { colour: part, share: null };
		};
		const one = split(first);
		const two = split(second);
		const p = one.share ?? (two.share === null ? 0.5 : 1 - two.share);
		const x = parseColour(one.colour);
		const y = parseColour(two.colour);
		if (!x || !y) return null;
		// Mixed with premultiplied alpha, as CSS does.
		const a = x.a * p + y.a * (1 - p);
		const channel = (key: "r" | "g" | "b") =>
			a === 0 ? 0 : (x[key] * x.a * p + y[key] * y.a * (1 - p)) / a;
		return { r: channel("r"), g: channel("g"), b: channel("b"), a };
	}
	return null;
};

/** A colour laid over an opaque one. */
export const over = (top: Rgba, under: Rgba): Rgba => ({
	r: top.r * top.a + under.r * (1 - top.a),
	g: top.g * top.a + under.g * (1 - top.a),
	b: top.b * top.a + under.b * (1 - top.a),
	a: 1,
});

/** WCAG's relative luminance of an opaque colour. */
export const luminance = ({ r, g, b }: Rgba): number =>
	0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);

/** WCAG's contrast ratio of two opaque colours, from 1 to 21. */
export const contrast = (one: Rgba, two: Rgba): number => {
	const [light, dark] = [luminance(one), luminance(two)].sort((x, y) => y - x);
	return (light! + 0.05) / (dark! + 0.05);
};
