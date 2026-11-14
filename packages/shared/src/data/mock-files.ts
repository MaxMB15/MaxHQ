// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sample files for the mock data source, so the file viewer has something to
// show in the browser preview: a Markdown plan, a picture, a PDF, a 3D part,
// a table, Rust and TypeScript sources, and a log with binary bytes in it. Each comes as a data URL, the way the media cache's URL comes
// in the app, and is made the first time it's asked for.

import { extensionOf } from "../agent-view/viewer-files";
import { POPOVER_PREVIEW } from "./mock-timeline";

/** A sample file's URL and its size in bytes. */
export type SampleFile = { readonly url: string; readonly size: number };

const PLAN = `# Usage popover

The popover lists the usage windows each account reports, in the order the
vendor sends them.

## What changes

- [x] Rows come from \`window_minutes\`, not a fixed pair
- [x] The sidebar meter shares the labels
- [ ] The limit wait card says which window ran out

| Window | Claude | Codex |
| ------ | -----: | ----: |
| 5 hours | not reported | 41% |
| Week | 63% | 18% |

> [!NOTE]
> An account that reports no window shows its plan name alone.

<details>
<summary>Why not keep the fixed rows?</summary>

Plans change their windows. A fixed row would show a bar for a window the
account doesn't have.

</details>
`;

const TABLE = `window,account,used,resets
5 hours,Codex,41%,2026-11-07 18:00
Week,Claude,63%,2026-11-10 09:00
Week,Codex,18%,2026-11-11 09:00
`;

const RUST = `//! Usage windows as each vendor reports them.

use std::time::Duration;

use serde::Deserialize;

/// One window an account reports, such as five hours or a week.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Window {
    /// How long the window lasts, in minutes.
    pub window_minutes: u32,
    /// How much of it is used, from 0 to 100.
    pub used_percent: f32,
    /// When it starts over, in seconds from now.
    pub resets_in: Option<u64>,
}

impl Window {
    /// The window's length.
    pub fn length(&self) -> Duration {
        Duration::from_secs(u64::from(self.window_minutes) * 60)
    }

    /// What the popover calls this window.
    pub fn label(&self) -> String {
        match self.window_minutes {
            300 => "5 hours".to_owned(),
            10_080 => "Week".to_owned(),
            minutes if minutes % 60 == 0 => format!("{} hours", minutes / 60),
            minutes => format!("{minutes} minutes"),
        }
    }
}

/// The windows worth a row: the ones the vendor sent, shortest first.
pub fn rows(windows: &[Window]) -> Vec<&Window> {
    let mut rows: Vec<&Window> = windows
        .iter()
        .filter(|window| window.window_minutes > 0)
        .collect();
    rows.sort_by_key(|window| window.window_minutes);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_week_is_labelled_as_one() {
        let week = Window {
            window_minutes: 10_080,
            used_percent: 63.0,
            resets_in: None,
        };
        assert_eq!(week.label(), "Week");
    }
}
`;

const TYPESCRIPT = `import type { UsageWindow } from "@/data/usage";

/** What the popover calls a window, from its length. */
export const windowLabel = (minutes: number): string => {
	if (minutes === 300) return "5 hours";
	if (minutes === 10_080) return "Week";
	return minutes % 60 === 0 ? \`\${minutes / 60} hours\` : \`\${minutes} minutes\`;
};

/** The rows the popover shows: the windows the vendor sent, shortest first. */
export const usageRows = (windows: readonly UsageWindow[]) =>
	windows
		.filter((window) => window.window_minutes > 0)
		.toSorted((a, b) => a.window_minutes - b.window_minutes)
		.map((window) => ({
			label: windowLabel(window.window_minutes),
			used: Math.round(window.used_percent),
			resets: window.resets_at ?? null,
		}));

// The sidebar meter shows the fullest window, so a nearly spent week
// isn't hidden behind a quiet five hours.
export const meterWindow = (windows: readonly UsageWindow[]): UsageWindow | null =>
	windows.reduce<UsageWindow | null>(
		(fullest, window) =>
			!fullest || window.used_percent > fullest.used_percent ? window : fullest,
		null
	);
`;

/**
 * A log the "Usage meter popover" agent read that has zero bytes in it, so
 * the viewer calls it binary.
 */
export const BINARY_SAMPLE = "logs/usage-poller.log";

// UTF-8 by hand: this package's globals leave out TextEncoder.
const utf8 = (text: string): Uint8Array =>
	Uint8Array.from(unescape(encodeURIComponent(text)), (char) => char.charCodeAt(0));

// How many bytes a base64 text holds, without decoding it.
const decodedSize = (text: string): number =>
	(text.length / 4) * 3 - (text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0);

const base64 = (bytes: Uint8Array): string => {
	let text = "";
	for (const byte of bytes) text += String.fromCharCode(byte);
	return btoa(text);
};

const dataUrl = (mime: string, bytes: Uint8Array): SampleFile => ({
	url: `data:${mime};base64,${base64(bytes)}`,
	size: bytes.byteLength,
});

/** A one-page PDF with a heading and two bars, its cross-reference table counted. */
const pdf = (): Uint8Array => {
	const page = [
		"BT /F1 20 Tf 56 760 Td (Usage popover mockup) Tj ET",
		"BT /F1 12 Tf 56 730 Td (Week) Tj ET",
		"0.38 0.55 0.95 rg 120 726 250 14 re f",
		"BT /F1 12 Tf 56 706 Td (5 hours) Tj ET",
		"0.38 0.55 0.95 rg 120 702 160 14 re f",
	].join("\n");
	const objects = [
		"<< /Type /Catalog /Pages 2 0 R >>",
		"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
		"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
		`<< /Length ${page.length} >>\nstream\n${page}\nendstream`,
		"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
	];
	let text = "%PDF-1.4\n";
	const offsets: number[] = [];
	objects.forEach((body, i) => {
		offsets.push(text.length);
		text += `${i + 1} 0 obj\n${body}\nendobj\n`;
	});
	const xref = text.length;
	text += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
	for (const at of offsets) text += `${String(at).padStart(10, "0")} 00000 n \n`;
	text += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
	return utf8(text);
};

type Box = { readonly min: readonly number[]; readonly max: readonly number[] };

// Each face of a box: the axis it faces along, which way, and its corners
// in order around it.
const FACES: readonly { normal: number[]; corners: number[][] }[] = [
	{
		normal: [1, 0, 0],
		corners: [
			[1, 0, 0],
			[1, 1, 0],
			[1, 1, 1],
			[1, 0, 1],
		],
	},
	{
		normal: [-1, 0, 0],
		corners: [
			[0, 0, 1],
			[0, 1, 1],
			[0, 1, 0],
			[0, 0, 0],
		],
	},
	{
		normal: [0, 1, 0],
		corners: [
			[0, 1, 1],
			[1, 1, 1],
			[1, 1, 0],
			[0, 1, 0],
		],
	},
	{
		normal: [0, -1, 0],
		corners: [
			[0, 0, 0],
			[1, 0, 0],
			[1, 0, 1],
			[0, 0, 1],
		],
	},
	{
		normal: [0, 0, 1],
		corners: [
			[0, 0, 1],
			[1, 0, 1],
			[1, 1, 1],
			[0, 1, 1],
		],
	},
	{
		normal: [0, 0, -1],
		corners: [
			[0, 1, 0],
			[1, 1, 0],
			[1, 0, 0],
			[0, 0, 0],
		],
	},
];

/** A binary glTF of a bracket made of boxes, one mesh with flat faces. */
const glb = (): Uint8Array => {
	const boxes: Box[] = [
		{ min: [-1, 0, -0.6], max: [1, 0.15, 0.6] },
		{ min: [-1, 0.15, -0.6], max: [-0.8, 1.1, 0.6] },
		{ min: [0.3, 0.15, -0.15], max: [0.6, 0.7, 0.15] },
	];
	const positions: number[] = [];
	const normals: number[] = [];
	const indices: number[] = [];
	for (const { min, max } of boxes) {
		for (const { normal, corners } of FACES) {
			const first = positions.length / 3;
			for (const corner of corners) {
				corner.forEach((side, axis) => positions.push(side ? max[axis]! : min[axis]!));
				normals.push(...normal);
			}
			indices.push(first, first + 1, first + 2, first, first + 2, first + 3);
		}
	}
	const count = positions.length / 3;
	const floats = new Float32Array([...positions, ...normals]);
	const shorts = new Uint16Array(indices);
	const indexBytes = shorts.byteLength + (shorts.byteLength % 4);
	const binary = new Uint8Array(floats.byteLength + indexBytes);
	binary.set(new Uint8Array(floats.buffer), 0);
	binary.set(new Uint8Array(shorts.buffer), floats.byteLength);
	const third = (axis: number) => positions.filter((_, i) => i % 3 === axis);
	const json = {
		asset: { version: "2.0" },
		scene: 0,
		scenes: [{ nodes: [0] }],
		nodes: [{ mesh: 0, name: "bracket" }],
		meshes: [
			{ primitives: [{ attributes: { POSITION: 0, NORMAL: 1 }, indices: 2, material: 0 }] },
		],
		materials: [
			{
				pbrMetallicRoughness: {
					baseColorFactor: [0.62, 0.66, 0.72, 1],
					metallicFactor: 0.6,
				},
			},
		],
		buffers: [{ byteLength: binary.byteLength }],
		bufferViews: [
			{ buffer: 0, byteOffset: 0, byteLength: count * 12, target: 34962 },
			{ buffer: 0, byteOffset: count * 12, byteLength: count * 12, target: 34962 },
			{ buffer: 0, byteOffset: count * 24, byteLength: shorts.byteLength, target: 34963 },
		],
		accessors: [
			{
				bufferView: 0,
				componentType: 5126,
				count,
				type: "VEC3",
				min: [0, 1, 2].map((axis) => Math.min(...third(axis))),
				max: [0, 1, 2].map((axis) => Math.max(...third(axis))),
			},
			{ bufferView: 1, componentType: 5126, count, type: "VEC3" },
			{ bufferView: 2, componentType: 5123, count: indices.length, type: "SCALAR" },
		],
	};
	let text = JSON.stringify(json);
	while (text.length % 4) text += " ";
	const head = utf8(text);
	const total = 12 + 8 + head.byteLength + 8 + binary.byteLength;
	const out = new Uint8Array(total);
	const view = new DataView(out.buffer);
	view.setUint32(0, 0x46546c67, true);
	view.setUint32(4, 2, true);
	view.setUint32(8, total, true);
	view.setUint32(12, head.byteLength, true);
	view.setUint32(16, 0x4e4f534a, true);
	out.set(head, 20);
	const bin = 20 + head.byteLength;
	view.setUint32(bin, binary.byteLength, true);
	view.setUint32(bin + 4, 0x004e4942, true);
	out.set(binary, bin + 8);
	return out;
};

const MAKERS: Readonly<Record<string, () => SampleFile>> = {
	md: () => dataUrl("text/markdown", utf8(PLAN)),
	csv: () => dataUrl("text/csv", utf8(TABLE)),
	png: () => ({ url: POPOVER_PREVIEW, size: decodedSize(POPOVER_PREVIEW.split(",")[1]!) }),
	pdf: () => dataUrl("application/pdf", pdf()),
	glb: () => dataUrl("model/gltf-binary", glb()),
	rs: () => dataUrl("text/plain", utf8(RUST)),
	ts: () => dataUrl("text/plain", utf8(TYPESCRIPT)),
	tsx: () => dataUrl("text/plain", utf8(TYPESCRIPT)),
};

/**
 * A note outside the "Usage meter popover" agent's folders. The device
 * refuses it until you click Show anyway.
 */
export const OUTSIDE_SAMPLE = "/tmp/example/usage-handoff.md";

const HANDOFF = `# Handoff notes

Written outside the project, so the agent read it from its own path.

- The popover change waits on the vendor's window list.
- Keep the sidebar meter's labels in the shared helper.
`;

const made = new Map<string, SampleFile>();

/** The sample file for a path the viewer shows, by its extension, or null for any other. */
export const sampleFile = (path: string): SampleFile | null => {
	if (path === OUTSIDE_SAMPLE) return dataUrl("text/markdown", utf8(HANDOFF));
	if (path === BINARY_SAMPLE) {
		return dataUrl("text/plain", Uint8Array.from([...utf8("poll 1 ok\n"), 0, 0, 7, 255, 0]));
	}
	const extension = extensionOf(path);
	const make = MAKERS[extension];
	if (!make) return null;
	let file = made.get(extension);
	if (!file) {
		file = make();
		made.set(extension, file);
	}
	return file;
};
