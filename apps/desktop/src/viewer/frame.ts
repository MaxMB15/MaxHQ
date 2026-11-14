// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The viewer frame's page. It runs in a sandboxed frame on an opaque origin
// with no network, waits for a file from the pane that holds it, and draws
// it with the file viewer's standard renderers and its 3D renderer, in the
// app's theme. Each renderer loads only when a file needs it.

import type { FileViewerRendererPluginInput } from "@file-viewer/core";
import { mountViewer, type ViewerController } from "@file-viewer/core/browser";

import { type FrameShown, isOpenFile, type OpenFile } from "./messages";

const target = document.getElementById("viewer")!;
let viewer: ViewerController | null = null;

const tell = (message: FrameShown) => window.parent.postMessage(message, "*");

const show = async ({ name, extension, bytes, theme }: OpenFile) => {
	document.documentElement.dataset.theme = theme;
	const [{ standardRenderers }, { modelRenderer }] = await Promise.all([
		import("@file-viewer/preset-standard"),
		import("@file-viewer/renderer-3d"),
	]);
	viewer?.destroy();
	viewer = mountViewer(target, {}, {});
	await viewer.load({
		buffer: bytes,
		name,
		type: extension,
		size: bytes.byteLength,
		options: {
			rendererMode: "replace",
			// The 3D renderer types its handler for the div it draws in,
			// which the list's wider element type doesn't take as is.
			renderers: [standardRenderers, modelRenderer as FileViewerRendererPluginInput],
			theme,
			locale: "en-US",
			toolbar: false,
			styleIsolation: "none",
			ai: false,
			watermark: false,
		},
	});
	const error = viewer.getState().error;
	if (error) throw error;
};

window.addEventListener("message", (event) => {
	if (event.source !== window.parent || !isOpenFile(event.data)) return;
	show(event.data).then(
		() => tell({ type: "maxhq-viewer:shown" }),
		(error: unknown) =>
			tell({
				type: "maxhq-viewer:failed",
				message: error instanceof Error ? error.message : String(error),
			})
	);
});

window.parent.postMessage({ type: "maxhq-viewer:ready" }, "*");
