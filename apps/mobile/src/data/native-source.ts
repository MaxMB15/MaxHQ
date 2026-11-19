// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The view from the phone's own core, over the bridge that
// uniffi-bindgen-react-native makes from crates/mobile. Every call carries
// the same JSON as the desktop's commands, so the requests and streams are
// the shared ones in request-source. Each reply comes back as a string,
// `{"status":"ok","data":...}` or `{"status":"error","error":...}`.
//
// The phone has one window, so every timeline and workflow run it opens
// shares one listener of its kind, as a desktop window's share one channel.
// Pictures come through the media cache in the core's cache folder, and
// downloads land in its downloads folder. Device setup and fleet updates
// aren't on the phone yet, and say so.

import { t } from "@maxhq/shared/i18n";

import type {
	ActionOutcome,
	AgentId,
	CommandError,
	CopiedProject,
	Delta,
	Downloaded,
	MediaFile,
	PhoneBook,
	PushToken,
	Request,
	Response,
	RunKey,
	TimelineMessage,
	TransferProgress,
	WorkflowMessage,
} from "@maxhq/shared/bindings";
import {
	asker,
	orThrow,
	requestCalls,
	timelines,
	toBase64,
	workflows,
	type Result,
} from "@maxhq/shared/data/request-source";
import type { DataSource, Unsubscribe } from "@maxhq/shared/data/source";
import type { PhoneNetwork } from "@maxhq/shared/status/phone-network";

/** Gets the view's changes, each call a JSON array of `Delta` oldest first. */
export type ViewListener = { changed: (deltas: string) => void };

/**
 * Gets one screen's batches, each a JSON array of `TimelineMessage` for a
 * timeline or of `WorkflowMessage` for a workflow run.
 */
export type TimelineListener = {
	id: () => number;
	batch: (messages: string) => boolean;
};

/** Hears how far a transfer has got, each `TransferProgress` as JSON. */
export type ProgressListener = { progress: (progress: string) => void };

/** The core's calls, as the generated module has them. Each reply is JSON. */
export type Bridge = {
	start: (dir: string, cache: string, platform: string) => string;
	request: (json: string) => Promise<string>;
	subscribe: (listener: ViewListener) => void;
	openTimeline: (agent: string, listener: TimelineListener) => Promise<string>;
	openWorkflow: (run: string, listener: TimelineListener) => Promise<string>;
	media: (
		agent: string,
		path: string,
		limit: number,
		listener: ProgressListener
	) => Promise<string>;
	upload: (
		device: string,
		target: string,
		path: string,
		listener: ProgressListener
	) => Promise<string>;
	uploadPasted: (
		device: string,
		target: string,
		name: string,
		mime: string,
		data: string,
		listener: ProgressListener
	) => Promise<string>;
	download: (agent: string, path: string, listener: ProgressListener) => Promise<string>;
	attachFromAgent: (
		device: string,
		target: string,
		agent: string,
		path: string,
		listener: ProgressListener
	) => Promise<string>;
	copyProject: (
		from: string,
		target: string,
		files: string,
		listener: ProgressListener
	) => Promise<string>;
	appState: (state: string) => string;
	networkChanged: (kind: string) => string;
	pushToken: (token: string) => string;
	presence: (json: string) => string;
	phoneKey: () => string;
	addDevice: (name: string, route: string, hostKey: string) => string;
	pair: (code: string, name: string) => Promise<string>;
	pairedMacs: () => string;
	forgetMac: (keeper: string) => string;
	pairCodeMac: (code: string) => string;
};

/** A device a Mac's book lists, as the phone shows it under that Mac. */
export type PairedDevice = {
	readonly device: string;
	/** The phone's link to it, as the view's `device_link` has it. */
	readonly link: string;
	readonly name: string;
	/** Whether another Mac's book lists it too, so forgetting this one keeps it. */
	readonly elsewhere: boolean;
};

/** A Mac the phone paired with, or the devices added by hand. */
export type PairedMac = {
	/** The device that keeps its book, which `forgetMac` takes. */
	readonly keeper: string;
	readonly name: string;
	/** Whether it holds the devices added by hand rather than a Mac's. */
	readonly by_hand: boolean;
	/** Each device once, under the first Mac that lists it. */
	readonly devices: readonly PairedDevice[];
};

/** The phone's key, and the `authorized_keys` line that lets it in. */
export type PhoneKey = {
	readonly phone: string;
	readonly public_key: string;
	readonly line: string;
	readonly push_public: string;
};

/** Whether the app is on screen. */
export type AppState = "active" | "background";

/** Reads a reply. One that isn't JSON comes back as a failure. */
export const readReply = <T>(json: string): Result<T> => {
	try {
		return JSON.parse(json) as Result<T>;
	} catch {
		return failed(t("The phone core answered with something that isn't JSON"));
	}
};

const failed = (message: string): { status: "error"; error: CommandError } => ({
	status: "error",
	error: { kind: "failed", message },
});

/** What a call the phone can't make yet answers. */
const notOnPhone = (what: "update devices" | "set devices up") =>
	failed(
		what === "update devices"
			? t("The phone can't update devices yet")
			: t("The phone can't set devices up yet")
	);

/** Hands each step of a transfer to `onProgress`. */
const progressTo = (onProgress: (progress: TransferProgress) => void): ProgressListener => ({
	progress: (json) => onProgress(JSON.parse(json) as TransferProgress),
});

/** A `file://` URL's path, as the core opens it. */
export const pathOf = (uri: string) =>
	decodeURIComponent(uri.replace(/^file:\/\//, "")).replace(/\/$/, "");

/** The `file://` URL of a path the core gave, such as a download's. */
export const urlOf = (path: string) =>
	`file://${path.split("/").map(encodeURIComponent).join("/")}`;

/**
 * A file in the media cache, as an image loads it. It's worked out on each
 * launch, since iOS can move the app's folders across an update.
 */
const mediaUrl = (cache: string, name: string) => `file://${encodeURI(`${cache}/media/${name}`)}`;

/** Each source's listener is a screen of its own, so a reload never reuses one. */
let screens = 0;

/** The phone's core as a data source, whose cache folder is `cache`. */
export const nativeSource = (bridge: Bridge, cache: string): DataSource => {
	const send = async (request: Request) =>
		readReply<Response>(await bridge.request(JSON.stringify(request)));
	const ask = asker(send);
	const screen = ++screens;

	// The core keeps one view listener, so the newest subscriber gets each
	// change and an unsubscribed one stops hearing them.
	let viewer = 0;
	const subscribe = async (onDelta: (delta: Delta) => void): Promise<Unsubscribe> => {
		const me = ++viewer;
		bridge.subscribe({
			changed: (json) => {
				if (viewer !== me) return;
				for (const delta of JSON.parse(json) as Delta[]) onDelta(delta);
			},
		});
		return () => {
			if (viewer === me) viewer = 0;
		};
	};

	return {
		...requestCalls(ask),
		kind: "native",
		subscribe,
		// A tap on a push opens its agent through the app's own navigation.
		onFocusAgent: async () => () => undefined,
		openTimeline: timelines(
			ask,
			(deliver): TimelineListener => ({
				id: () => screen,
				batch: (json) => {
					deliver(JSON.parse(json) as TimelineMessage[]);
					return true;
				},
			}),
			async (agent: AgentId, listener) =>
				readReply<null>(await bridge.openTimeline(agent, listener))
		),
		openWorkflow: workflows(
			ask,
			(deliver): TimelineListener => ({
				id: () => screen,
				batch: (json) => {
					deliver(JSON.parse(json) as WorkflowMessage[]);
					return true;
				},
			}),
			async (run: RunKey, listener) =>
				readReply<null>(await bridge.openWorkflow(JSON.stringify(run), listener))
		),
		copyProject: async (from, target, files, onProgress) =>
			orThrow(
				readReply<CopiedProject>(
					await bridge.copyProject(
						from,
						JSON.stringify(target),
						JSON.stringify(files),
						progressTo(onProgress)
					)
				)
			),
		// One file at a time, each a path in the app's caches folder, where
		// the pickers copy what you pick.
		upload: async (device, target, paths, onProgress) => {
			const results = [];
			for (const path of paths) {
				results.push(
					readReply<ActionOutcome>(
						await bridge.upload(
							device,
							JSON.stringify(target),
							path,
							progressTo(onProgress)
						)
					)
				);
			}
			return results;
		},
		uploadPasted: async (device, target, name, mime, bytes, onProgress) =>
			readReply<ActionOutcome>(
				await bridge.uploadPasted(
					device,
					JSON.stringify(target),
					name,
					mime,
					toBase64(bytes),
					progressTo(onProgress)
				)
			),
		attachFromAgent: async (device, target, from, path, onProgress) =>
			readReply<ActionOutcome>(
				await bridge.attachFromAgent(
					device,
					JSON.stringify(target),
					from,
					path,
					progressTo(onProgress)
				)
			),
		download: async (agent, path, onProgress) =>
			readReply<Downloaded>(await bridge.download(agent, path, progressTo(onProgress))),
		// A phone never asks for a file outside the agent's folders, so the
		// core leaves Show anyway off whatever `anyway` says.
		media: async (agent, path, limit, _anyway, onProgress) => {
			const result = readReply<MediaFile>(
				await bridge.media(agent, path, limit, progressTo(onProgress))
			);
			if (result.status === "error") return result;
			const file = result.data;
			return {
				status: "ok",
				data:
					file.kind === "cached"
						? { kind: "cached", url: mediaUrl(cache, file.name), size: file.size }
						: file,
			};
		},
		updateFleet: async () => notOnPhone("update devices"),
		setupDevice: async () => notOnPhone("set devices up"),
	};
};

/** The calls only the phone has: its life cycle, network and key. */
export const phoneCalls = (bridge: Bridge) => ({
	/** Opens the core on the app's data and cache folders. Safe to call again after a reload. */
	start: (dir: string, cache: string) =>
		orThrow(readReply<null>(bridge.start(dir, cache, "ios"))),
	/** Tells the core the app came to the screen or left it. */
	appState: (state: AppState) => readReply<null>(bridge.appState(state)),
	/** Tells the core the phone changed networks. */
	networkChanged: (kind: PhoneNetwork) => readReply<null>(bridge.networkChanged(kind)),
	/** Hands the core the push token and kinds, which it gives every device. */
	pushToken: (token: PushToken) => readReply<null>(bridge.pushToken(JSON.stringify(token))),
	/** Tells the core which agent is on screen, so no device pushes about it. */
	presence: (agent: AgentId | null) =>
		readReply<null>(bridge.presence(JSON.stringify({ agent }))),
	/** The phone's key and its `authorized_keys` line. */
	phoneKey: () => readReply<PhoneKey>(bridge.phoneKey()),
	/** Adds a device by hand: a name, `user@host:port`, and its host key line. */
	addDevice: (name: string, route: string, hostKey: string) =>
		readReply<null>(bridge.addDevice(name, route, hostKey)),
	/** Pairs through the device whose code a Mac shows, as `name`. */
	pair: async (code: string, name: string) => readReply<PhoneBook>(await bridge.pair(code, name)),
	/** The Macs the phone paired with, in the order it paired, the devices added by hand last. */
	pairedMacs: () => readReply<PairedMac[]>(bridge.pairedMacs()),
	/** Forgets one Mac and the devices only it lists. The other Macs stay paired. */
	forgetMac: (keeper: string) => readReply<null>(bridge.forgetMac(keeper)),
	/** The name of the paired Mac a code comes from, or null for a new Mac. */
	pairCodeMac: (code: string) => readReply<string | null>(bridge.pairCodeMac(code)),
});

export type PhoneCalls = ReturnType<typeof phoneCalls>;
