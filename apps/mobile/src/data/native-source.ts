// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The view from the phone's own core, over the bridge that
// uniffi-bindgen-react-native makes from crates/mobile. Every call carries
// the same JSON as the desktop's commands, so the requests and streams are
// the shared ones in request-source. Each reply comes back as a string,
// `{"status":"ok","data":...}` or `{"status":"error","error":...}`.
//
// The phone has one window, so every timeline it opens shares one listener,
// as a desktop window's share one channel. Pictures come through the media
// cache in the core's cache folder. Other transfers, workflow runs and device
// setup aren't on the phone yet, and say so.

import { t } from "@maxhq/shared/i18n";

import type {
	AgentId,
	CommandError,
	Delta,
	MediaFile,
	PhoneBook,
	PushToken,
	Request,
	Response,
	TimelineMessage,
	TransferProgress,
} from "@maxhq/shared/bindings";
import {
	asker,
	describe,
	orThrow,
	requestCalls,
	timelines,
	type Result,
} from "@maxhq/shared/data/request-source";
import type { DataSource, Unsubscribe } from "@maxhq/shared/data/source";
import type { PhoneNetwork } from "@maxhq/shared/status/phone-network";

/** Gets the view's changes, each call a JSON array of `Delta` oldest first. */
export type ViewListener = { changed: (deltas: string) => void };

/** Gets one screen's timeline batches, each a JSON array of `TimelineMessage`. */
export type TimelineListener = {
	id: () => number;
	batch: (messages: string) => boolean;
};

/** Hears how far a picture has got, each `TransferProgress` as JSON. */
export type MediaListener = { progress: (progress: string) => void };

/** The core's calls, as the generated module has them. Each reply is JSON. */
export type Bridge = {
	start: (dir: string, cache: string, platform: string) => string;
	request: (json: string) => Promise<string>;
	subscribe: (listener: ViewListener) => void;
	openTimeline: (agent: string, listener: TimelineListener) => Promise<string>;
	media: (agent: string, path: string, limit: number, listener: MediaListener) => Promise<string>;
	appState: (state: string) => string;
	networkChanged: (kind: string) => string;
	pushToken: (token: string) => string;
	presence: (json: string) => string;
	phoneKey: () => string;
	addDevice: (name: string, route: string, hostKey: string) => string;
	pair: (code: string, name: string) => Promise<string>;
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
const notOnPhone = (what: string) =>
	failed(
		t(
			(
				{
					"show workflow runs": "The phone can't show workflow runs yet",
					"copy projects": "The phone can't copy projects yet",
					"send files": "The phone can't send files yet",
					"download files": "The phone can't download files yet",
					"update devices": "The phone can't update devices yet",
					"set devices up": "The phone can't set devices up yet",
				} as Record<string, string>
			)[what] ?? "The phone can't {{action}} yet",
			{ action: what }
		)
	);

/** Rejects, for a call that resolves with its value. */
const refuse = async (what: string): Promise<never> => {
	const { error } = notOnPhone(what);
	throw new Error(describe(error), { cause: error });
};

/** A `file://` URL's path, as the core opens it. */
export const pathOf = (uri: string) =>
	decodeURIComponent(uri.replace(/^file:\/\//, "")).replace(/\/$/, "");

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
		openWorkflow: () => refuse("show workflow runs"),
		copyProject: () => refuse("copy projects"),
		upload: async (_device, _target, paths) => paths.map(() => notOnPhone("send files")),
		uploadPasted: async () => notOnPhone("send files"),
		attachFromAgent: async () => notOnPhone("send files"),
		download: async () => notOnPhone("download files"),
		// A phone never asks for a file outside the agent's folders, so the
		// core leaves Show anyway off whatever `anyway` says.
		media: async (agent, path, limit, _anyway, onProgress) => {
			const result = readReply<MediaFile>(
				await bridge.media(agent, path, limit, {
					progress: (json) => onProgress(JSON.parse(json) as TransferProgress),
				})
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
	/** Pairs through the device whose code the Mac shows, as `name`. */
	pair: async (code: string, name: string) => readReply<PhoneBook>(await bridge.pair(code, name)),
});

export type PhoneCalls = ReturnType<typeof phoneCalls>;
