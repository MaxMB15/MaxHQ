// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The view from the app's backend, over the commands and events that
// tauri-specta generates into bindings.ts. The requests and streams are the
// shared ones in request-source. Here they go over Tauri's commands, and
// every timeline a window opens shares one channel, which the backend fills
// in batches of at most one a frame, as does every workflow run. Each upload
// or download has a channel of its own for its progress. A picture or video
// the timeline shows lands in the app's media cache, and the webview loads
// it from there through the app's media scheme.

import { Channel, convertFileSrc } from "@tauri-apps/api/core";

import { commands, events } from "./tauri-commands";
import type {
	FleetProgress,
	SetupProgress,
	TimelineMessage,
	TransferProgress,
	WorkflowMessage,
} from "@maxhq/shared/bindings";

import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import {
	asker,
	orThrow,
	requestCalls,
	timelines,
	toBase64,
	workflows,
} from "@maxhq/shared/data/request-source";

import { terminalCalls } from "@maxhq/shared/terminal/requests";

import { tauriStream } from "@/terminal/tauri-stream";

export { MAX_PASTE, toBase64 } from "@maxhq/shared/data/request-source";

/** The scheme the app serves its media cache through. */
const MEDIA_SCHEME = "maxmedia";

/** The command takes a 32-bit limit, which is more than the cache takes anyway. */
const MOST_LIMIT = 0xffff_ffff;

/** A channel for one transfer that hands each step to `onProgress`. */
const progressTo = (onProgress: (progress: TransferProgress) => void) =>
	new Channel<TransferProgress>(onProgress);

const ask = asker(commands.request);

/** The app and protocol versions. */
export const appInfo = async () => orThrow(await ask({ kind: "app_info" }));

/** The backend as a data source. */
export const tauriSource = (): DataSource => ({
	...requestCalls(ask),
	kind: "tauri",
	terminalStream: tauriStream(terminalCalls(ask).resizeTerminal),
	subscribe: (onDelta) =>
		events.viewChanged.listen((event) => {
			for (const delta of event.payload) onDelta(delta);
		}),
	onFocusAgent: (onFocus) => events.focusAgent.listen((event) => onFocus(event.payload.agent)),
	openTimeline: timelines(
		ask,
		(deliver) => new Channel<TimelineMessage[]>(deliver),
		(agent, channel) => commands.openTimeline(agent, channel)
	),
	openWorkflow: workflows(
		ask,
		(deliver) => new Channel<WorkflowMessage[]>(deliver),
		(run, channel) => commands.openWorkflow(run, channel)
	),
	copyProject: async (from, target, files, onProgress) =>
		orThrow(await commands.copyProject(from, target, files, progressTo(onProgress))),
	// One path at a time, so each has the link to itself and a progress
	// bar that fills in order.
	upload: async (device, target, paths, onProgress) => {
		const results: ActionResult[] = [];
		for (const path of paths) {
			results.push(await commands.upload(device, target, path, progressTo(onProgress)));
		}
		return results;
	},
	uploadPasted: (device, target, name, mime, bytes, onProgress) =>
		commands.uploadPasted(device, target, name, mime, toBase64(bytes), progressTo(onProgress)),
	attachFromAgent: (device, target, from, path, onProgress) =>
		commands.attachFromAgent(device, target, from, path, progressTo(onProgress)),
	download: (agent, path, onProgress) => commands.download(agent, path, progressTo(onProgress)),
	media: async (agent, path, limit, anyway, onProgress) => {
		const bounded = Math.min(MOST_LIMIT, Math.max(0, Math.floor(limit)));
		const result = await commands.media(agent, path, bounded, anyway, progressTo(onProgress));
		if (result.status === "error") return result;
		const file = result.data;
		return {
			status: "ok",
			data:
				file.kind === "cached"
					? {
							kind: "cached",
							url: convertFileSrc(file.name, MEDIA_SCHEME),
							size: file.size,
						}
					: file,
		};
	},
	updateFleet: (devices, tools, unsigned, onProgress) =>
		commands.updateFleet(devices, tools, unsigned, new Channel<FleetProgress>(onProgress)),
	setupDevice: (host, secrets, onProgress) =>
		commands.setupDevice(host, secrets, new Channel<SetupProgress>(onProgress)),
});
