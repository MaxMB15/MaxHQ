// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PhoneBook, Request, TimelineMessage, TimelineUpdate } from "@maxhq/shared/bindings";
import {
	ACTIONS,
	answered,
	DONE,
	events,
	OK,
	reset,
	SPEC,
	UNITS,
} from "@maxhq/shared/test/requests";

import {
	type Bridge,
	nativeSource,
	type PairedMac,
	pathOf,
	phoneCalls,
	type TimelineListener,
	type ViewListener,
} from "./native-source";

/** The data folder the core opens on. */
const CACHE = "/phone/Library/Caches/maxhq";

/** The book a pairing answers. */
const BOOK: PhoneBook = { phone: "phone-1", device: "device-1", devices: [] };

/** The Macs the core says the phone paired with. */
const MACS: PairedMac[] = [
	{
		keeper: "home-mac",
		name: "Home Mac",
		by_hand: false,
		devices: [{ device: "devbox", link: "link-devbox", name: "devbox", elsewhere: false }],
	},
];

/** A bridge that answers each request with a response of its kind. */
const fakeBridge = () => {
	const sent: Request[] = [];
	const timelines: TimelineListener[] = [];
	const viewers: ViewListener[] = [];
	let answer = (request: Request): unknown => answered(request.kind);
	const json = (value: unknown) => JSON.stringify(value);
	const bridge: Bridge = {
		start: vi.fn(() => json(OK)),
		request: async (body) => {
			const request = JSON.parse(body) as Request;
			sent.push(request);
			return json(answer(request));
		},
		subscribe: (listener) => void viewers.push(listener),
		openTimeline: async (_agent, listener) => {
			timelines.push(listener);
			return json(OK);
		},
		media: vi.fn(async () => json({ status: "ok", data: { kind: "too_large", size: 9 } })),
		appState: vi.fn(() => json(OK)),
		networkChanged: vi.fn(() => json(OK)),
		pushToken: vi.fn(() => json(OK)),
		presence: vi.fn(() => json(OK)),
		phoneKey: () =>
			json({
				status: "ok",
				data: { phone: "phone-1", public_key: "k", line: "l", push_public: "p" },
			}),
		addDevice: vi.fn(() => json(OK)),
		pair: vi.fn(async () => json({ status: "ok", data: BOOK })),
		pairedMacs: () => json({ status: "ok", data: MACS }),
		forgetMac: vi.fn(() => json(OK)),
		pairCodeMac: vi.fn(() => json({ status: "ok", data: "Home Mac" })),
	};
	const answerWith = (next: (request: Request) => unknown) => (answer = next);
	return { bridge, sent, timelines, viewers, answerWith };
};

let fake: ReturnType<typeof fakeBridge>;

beforeEach(() => {
	fake = fakeBridge();
});

/** Sends one batch down the phone's timeline listener. */
// Events are live unless a message says otherwise.
const send = (...batch: (Omit<TimelineMessage, "live"> & { live?: boolean })[]) => {
	const [listener] = fake.timelines;
	if (!listener) throw new Error("no timeline listener yet");
	const live = batch.map((message) => ({
		...message,
		live: message.live ?? message.update.type === "events",
	}));
	return listener.batch(JSON.stringify(live));
};

describe("native source", () => {
	it.each(ACTIONS)(
		"sends %s as one request and hands back what came of it",
		async (_, call, request) => {
			fake.answerWith((asked) => answered(asked.kind, { status: "done" }));
			await expect(call(nativeSource(fake.bridge, CACHE))).resolves.toEqual(DONE);
			expect(fake.sent).toEqual([request]);
		}
	);

	it.each(UNITS)(
		"sends %s as one request that answers with nothing",
		async (_, call, request) => {
			await expect(call(nativeSource(fake.bridge, CACHE))).resolves.toEqual(OK);
			expect(fake.sent).toEqual([request]);
		}
	);

	it("hands back a failure as it came, and one that isn't JSON as a failure", async () => {
		const failed = { status: "error", error: { kind: "not_connected", device: "devbox" } };
		fake.answerWith(() => failed);
		await expect(nativeSource(fake.bridge, CACHE).startAgent(SPEC)).resolves.toEqual(failed);

		fake.bridge.request = async () => "not json";
		await expect(nativeSource(fake.bridge, CACHE).startAgent(SPEC)).resolves.toMatchObject({
			status: "error",
			error: { kind: "failed" },
		});
	});

	it("rejects a response of another kind", async () => {
		fake.answerWith(() => answered("stop_agent", { status: "done" }));
		await expect(nativeSource(fake.bridge, CACHE).startAgent(SPEC)).rejects.toThrow(
			"A start_agent request came back as stop_agent"
		);
	});

	it("hands each change to the newest subscriber until it unsubscribes", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		const first: unknown[] = [];
		const second: unknown[] = [];
		await source.subscribe((delta) => first.push(delta));
		const stop = await source.subscribe((delta) => second.push(delta));
		const delta = { seq: 1, changes: [] };
		for (const viewer of fake.viewers) viewer.changed(JSON.stringify([delta]));
		expect(first).toEqual([]);
		expect(second).toEqual([delta]);
		stop();
		fake.viewers.at(-1)?.changed(JSON.stringify([delta]));
		expect(second).toEqual([delta]);
	});

	it("hands each change of a batch over in order", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		const heard: unknown[] = [];
		await source.subscribe((delta) => heard.push(delta));
		const batch = [
			{ seq: 1, changes: [] },
			{ seq: 2, changes: [] },
		];
		fake.viewers.at(-1)?.changed(JSON.stringify(batch));
		expect(heard).toEqual(batch);
	});

	it("says plainly what the phone can't do yet", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		const refused = { status: "error", error: { kind: "failed", message: expect.any(String) } };
		const target = { type: "agent", agent: "agent-1" } as const;
		await expect(source.download("agent-1", "/a", () => undefined)).resolves.toEqual(refused);
		await expect(
			source.upload("devbox", target, ["/a", "/b"], () => undefined)
		).resolves.toEqual([refused, refused]);
		await expect(
			source.openWorkflow({ device: "devbox", run: "run-1" } as never, () => undefined)
		).rejects.toThrow("The phone can't show workflow runs yet");
		expect(fake.sent).toEqual([]);
	});
});

describe("native source pictures", () => {
	const fetched = (reply: unknown) =>
		vi.mocked(fake.bridge.media).mockResolvedValueOnce(JSON.stringify(reply));

	it("finds a cached picture in the cache folder's media cache", async () => {
		fetched({ status: "ok", data: { kind: "cached", name: "ab12.png", size: 42 } });
		const source = nativeSource(fake.bridge, "/phone/Library/My Caches/maxhq");
		await expect(
			source.media("agent-1", "shot.png", 1024, false, () => undefined)
		).resolves.toEqual({
			status: "ok",
			data: {
				kind: "cached",
				url: "file:///phone/Library/My%20Caches/maxhq/media/ab12.png",
				size: 42,
			},
		});
		expect(fake.bridge.media).toHaveBeenCalledWith(
			"agent-1",
			"shot.png",
			1024,
			expect.anything()
		);
	});

	it("passes on one too large, and a failure as it came", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		await expect(
			source.media("agent-1", "big.png", 1024, false, () => undefined)
		).resolves.toEqual({
			status: "ok",
			data: { kind: "too_large", size: 9 },
		});
		const refused = { status: "error", error: { kind: "failed", message: "no" } };
		fetched(refused);
		await expect(
			source.media("agent-1", "shot.png", 1024, false, () => undefined)
		).resolves.toEqual(refused);
	});

	it("reads each step of progress", async () => {
		const step = { transfer: "t-1", sent: 5, total: 10, file: "shot.png", state: "running" };
		vi.mocked(fake.bridge.media).mockImplementationOnce(
			async (_agent, _path, _limit, listener) => {
				listener.progress(JSON.stringify(step));
				return JSON.stringify({ status: "ok", data: { kind: "too_large", size: 10 } });
			}
		);
		const heard = vi.fn();
		await nativeSource(fake.bridge, CACHE).media("agent-1", "shot.png", 0, false, heard);
		expect(heard).toHaveBeenCalledWith(step);
	});
});

describe("native source timelines", () => {
	const update = (seq: number): TimelineUpdate => events(seq);

	it("opens every timeline over the phone's one listener and routes by agent", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		const one: TimelineUpdate[] = [];
		const two: TimelineUpdate[] = [];
		await source.openTimeline("agent-1", (u) => one.push(u));
		await source.openTimeline("agent-2", (u) => two.push(u));
		const [first, second] = fake.timelines;
		expect(first).toBe(second);
		expect(
			send(
				{ agent: "agent-1", update: reset(1) },
				{ agent: "agent-2", update: update(1) },
				{ agent: "agent-2", update: reset(2) },
				{ agent: "agent-1", update: update(2) }
			)
		).toBe(true);
		expect(one).toEqual([reset(1), update(2)]);
		expect(two).toEqual([reset(2)]);
	});

	it("closes once for each open", async () => {
		const source = nativeSource(fake.bridge, CACHE);
		const close = await source.openTimeline("agent-1", () => undefined);
		fake.sent.length = 0;
		close();
		close();
		await Promise.resolve();
		expect(fake.sent).toEqual([{ kind: "close_timeline", agent: "agent-1" }]);
	});

	it("gives each source a screen of its own", async () => {
		const first = nativeSource(fake.bridge, CACHE);
		const second = nativeSource(fake.bridge, CACHE);
		await first.openTimeline("agent-1", () => undefined);
		await second.openTimeline("agent-1", () => undefined);
		const [a, b] = fake.timelines;
		expect(a?.id()).not.toBe(b?.id());
	});
});

describe("phone calls", () => {
	it("opens the core on a file URL's path", () => {
		expect(pathOf("file:///var/mobile/Data/My%20App/Documents/")).toBe(
			"/var/mobile/Data/My App/Documents"
		);
	});

	it("starts on the data and cache folders and passes the app's state and network on", () => {
		const phone = phoneCalls(fake.bridge);
		expect(phone.start("/data", "/cache")).toBeNull();
		expect(fake.bridge.start).toHaveBeenCalledWith("/data", "/cache", "ios");
		expect(phone.appState("background")).toEqual(OK);
		expect(fake.bridge.appState).toHaveBeenCalledWith("background");
		expect(phone.networkChanged("cellular")).toEqual(OK);
		expect(fake.bridge.networkChanged).toHaveBeenCalledWith("cellular");
	});

	it("passes the push token and the agent on screen on as JSON", () => {
		const phone = phoneCalls(fake.bridge);
		const token = {
			token: "test-not-a-real-token",
			topic: "com.example.maxhq",
			environment: "sandbox",
			kinds: ["needs_input"],
		} as const;
		expect(phone.pushToken({ ...token, kinds: [...token.kinds] })).toEqual(OK);
		expect(fake.bridge.pushToken).toHaveBeenCalledWith(JSON.stringify(token));
		expect(phone.presence("agent-1")).toEqual(OK);
		expect(fake.bridge.presence).toHaveBeenCalledWith('{"agent":"agent-1"}');
		expect(phone.presence(null)).toEqual(OK);
		expect(fake.bridge.presence).toHaveBeenCalledWith('{"agent":null}');
	});

	it("throws when the core doesn't start", () => {
		const error = { kind: "failed", message: "The phone core hasn't started." };
		fake.bridge.start = () => JSON.stringify({ status: "error", error });
		expect(() => phoneCalls(fake.bridge).start("/data", "/cache")).toThrow(error.message);
	});

	it("reads the phone's key and adds a device by hand", () => {
		const phone = phoneCalls(fake.bridge);
		expect(phone.phoneKey()).toEqual({
			status: "ok",
			data: { phone: "phone-1", public_key: "k", line: "l", push_public: "p" },
		});
		expect(phone.addDevice("devbox", "dev@devbox:22", "ssh-ed25519 AAAA")).toEqual(OK);
		expect(fake.bridge.addDevice).toHaveBeenCalledWith(
			"devbox",
			"dev@devbox:22",
			"ssh-ed25519 AAAA"
		);
	});

	it("pairs with the code it scanned and answers the book", async () => {
		const phone = phoneCalls(fake.bridge);
		await expect(phone.pair("maxhq:AQ", "iPhone")).resolves.toEqual({
			status: "ok",
			data: BOOK,
		});
		expect(fake.bridge.pair).toHaveBeenCalledWith("maxhq:AQ", "iPhone");
	});

	it("lists the Macs, forgets one by its keeper and names a code's Mac", () => {
		const phone = phoneCalls(fake.bridge);
		expect(phone.pairedMacs()).toEqual({ status: "ok", data: MACS });
		expect(phone.forgetMac("home-mac")).toEqual(OK);
		expect(fake.bridge.forgetMac).toHaveBeenCalledWith("home-mac");
		expect(phone.pairCodeMac("maxhq:AQ")).toEqual({ status: "ok", data: "Home Mac" });
		expect(fake.bridge.pairCodeMac).toHaveBeenCalledWith("maxhq:AQ");
	});
});
