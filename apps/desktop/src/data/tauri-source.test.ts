// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
	AccessKey,
	Request,
	Run,
	TaskDelivery,
	TimelineMessage,
	TimelineUpdate,
	UploadTarget,
} from "@maxhq/shared/bindings";

import type { TransferProgress } from "@maxhq/shared/data/source";
import { leaseScreenOf } from "@maxhq/shared/test-hosts/format";
import {
	ACTIONS,
	answered,
	DONE,
	events,
	NO_LIMITS,
	OK,
	PERMISSION,
	reset,
	SPEC,
	TASK,
	UNITS,
} from "@maxhq/shared/test/requests";
import { MAX_PASTE, tauriSource, toBase64 } from "./tauri-source";

const mocks = vi.hoisted(() => {
	/** Stands in for the runtime's channel, keeping the handler to call. */
	class FakeChannel {
		static made: FakeChannel[] = [];
		constructor(public onmessage: (message: unknown) => void) {
			FakeChannel.made.push(this);
		}
	}
	const names = [
		"request",
		"openTimeline",
		"upload",
		"uploadPasted",
		"download",
		"media",
		"updateFleet",
		"setupDevice",
	] as const;
	const commands = Object.fromEntries(names.map((name) => [name, vi.fn()])) as Record<
		(typeof names)[number],
		ReturnType<typeof vi.fn>
	>;
	return { FakeChannel, commands };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: mocks.FakeChannel,
	convertFileSrc: (name: string, scheme: string) => `${scheme}://localhost/${name}`,
}));
vi.mock("./tauri-commands", () => ({
	commands: mocks.commands,
	events: {},
}));

const { FakeChannel, commands } = mocks;

/**
 * Sends one batch down the window's timeline channel. Events are live
 * unless a message says otherwise.
 */
const send = (...batch: (Omit<TimelineMessage, "live"> & { live?: boolean })[]) => {
	const [channel, ...more] = FakeChannel.made;
	expect(more).toEqual([]);
	if (!channel) throw new Error("no timeline channel yet");
	channel.onmessage(
		batch.map((message) => ({
			...message,
			live: message.live ?? message.update.type === "events",
		}))
	);
};

beforeEach(() => {
	FakeChannel.made = [];
	for (const command of Object.values(commands)) {
		command.mockReset();
		command.mockResolvedValue(OK);
	}
	commands.request.mockImplementation(async (request: Request) => answered(request.kind));
});

const TO_AGENT: UploadTarget = { type: "agent", agent: "agent-1" };

/** A step of a transfer, as its channel hears it. */
const step = (state: TransferProgress["state"]): TransferProgress => ({
	transfer: "transfer-1",
	sent: state === "done" ? 4 : 0,
	total: 4,
	file: "a.txt",
	state,
});

describe("tauri source", () => {
	it.each(ACTIONS)(
		"sends %s as one request and hands back what came of it",
		async (_, call, request) => {
			commands.request.mockResolvedValue(answered(request.kind, { status: "done" }));
			await expect(call(tauriSource())).resolves.toEqual(DONE);
			expect(commands.request).toHaveBeenCalledExactlyOnceWith(request);
		}
	);

	it.each(UNITS)(
		"sends %s as one request that answers with nothing",
		async (_, call, request) => {
			await expect(call(tauriSource())).resolves.toEqual(OK);
			expect(commands.request).toHaveBeenCalledExactlyOnceWith(request);
		}
	);

	it("hands back a request that failed as it came", async () => {
		const failed = { status: "error", error: { kind: "not_connected", device: "nl" } };
		commands.request.mockResolvedValue(failed);
		await expect(tauriSource().startAgent(SPEC)).resolves.toEqual(failed);
	});

	it("rejects a response of another kind", async () => {
		commands.request.mockResolvedValue(answered("stop_agent", { status: "done" }));
		await expect(tauriSource().startAgent(SPEC)).rejects.toThrow(
			"A start_agent request came back as stop_agent"
		);
	});

	it("unwraps what only fails when the backend does", async () => {
		const snapshot = { seq: 3, entities: [] };
		const info = { version: "0.1.0", protocol_version: 1 };
		const identity = { name: "Max", email: null };
		const paths = [{ path: "src/main.rs", kind: "file" }];
		const search = { repo: "owner/repo", items: [], unavailable: null };
		const excerpt = { name: "agent-api.md", text: "# Agent: API\n", messages: 1, omitted: 0 };
		commands.request.mockImplementation(async (request: Request) => {
			switch (request.kind) {
				case "view_snapshot":
					return answered(request.kind, snapshot);
				case "app_info":
					return answered(request.kind, info);
				case "git_identity":
					return answered(request.kind, identity);
				case "find_paths":
					return answered(request.kind, paths);
				case "find_git_items":
					return answered(request.kind, search);
				case "excerpt_agent":
					return answered(request.kind, excerpt);
				default:
					return answered(request.kind);
			}
		});
		const source = tauriSource();
		await expect(source.snapshot()).resolves.toEqual(snapshot);
		await expect(source.appInfo()).resolves.toEqual(info);
		await expect(source.gitIdentity()).resolves.toEqual(identity);
		await expect(source.findPaths("agent-1", "src/ma")).resolves.toEqual(paths);
		await expect(source.findGitItems("agent-1", "12")).resolves.toEqual(search);
		await expect(source.findGitItems("agent-1", "7", "someone/else")).resolves.toEqual(search);
		await expect(source.excerptAgent("agent-1", 20000)).resolves.toEqual(excerpt);
		await expect(
			source.setShown({ focused: "agent-1", visible: ["agent-1"] })
		).resolves.toBeUndefined();
		await expect(source.cancelTransfer("transfer-1")).resolves.toBeUndefined();
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "view_snapshot" }],
			[{ kind: "app_info" }],
			[{ kind: "git_identity" }],
			[{ kind: "find_paths", agent: "agent-1", query: "src/ma" }],
			[{ kind: "find_git_items", agent: "agent-1", query: "12", repo: null }],
			[{ kind: "find_git_items", agent: "agent-1", query: "7", repo: "someone/else" }],
			[{ kind: "excerpt_agent", agent: "agent-1", budget: 20000 }],
			[{ kind: "set_shown", shown: { focused: "agent-1", visible: ["agent-1"] } }],
			[{ kind: "cancel_transfer", transfer: "transfer-1" }],
		]);

		const error = { kind: "failed", message: "The backend went away" };
		commands.request.mockResolvedValue({ status: "error", error });
		await expect(source.snapshot()).rejects.toHaveProperty("cause", error);
	});

	it("sends what a window shows in the order the page said it", async () => {
		let answer: (value: unknown) => void = () => undefined;
		commands.request.mockReturnValueOnce(new Promise((resolve) => (answer = resolve)));
		commands.request.mockResolvedValue(answered("set_shown"));
		const source = tauriSource();
		const first = source.setShown({ focused: "agent-1", visible: ["agent-1"] });
		const second = source.setShown({ focused: null, visible: [] });
		await Promise.resolve();
		expect(commands.request).toHaveBeenCalledTimes(1);
		answer(answered("set_shown"));
		await Promise.all([first, second]);
		expect(commands.request.mock.calls.map(([request]) => request.shown)).toEqual([
			{ focused: "agent-1", visible: ["agent-1"] },
			{ focused: null, visible: [] },
		]);
	});

	it("opens a forward and gives the port it listens on here", async () => {
		const opened = { forward: "forward-1", local_port: 51234 };
		commands.request.mockResolvedValue(answered("open_forward", opened));
		await expect(tauriSource().openForward("port-1")).resolves.toEqual({
			status: "ok",
			data: opened,
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "open_forward",
			port: "port-1",
		});
	});

	it("stages offered paths with one result per path", async () => {
		const replies = [
			DONE,
			{ status: "error", error: { kind: "failed", message: "No such file" } },
		];
		commands.request.mockResolvedValue(answered("stage_offered", replies));
		await expect(tauriSource().stageAttachments("device-1", ["/a", "/b"])).resolves.toEqual(
			replies
		);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "stage_offered",
			device: "device-1",
			paths: ["/a", "/b"],
		});
	});

	it("stages what the @ menu found by the agent and its paths", async () => {
		commands.request.mockResolvedValue(answered("stage_found", [DONE]));
		await expect(tauriSource().stageFound("agent-1", ["src/main.rs"])).resolves.toEqual([DONE]);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "stage_found",
			agent: "agent-1",
			paths: ["src/main.rs"],
		});
	});

	it("sends a paste as base64", async () => {
		commands.request.mockResolvedValue(answered("stage_pasted", { status: "done" }));
		const bytes = new TextEncoder().encode("hi");
		await expect(
			tauriSource().stagePasted("device-1", "a.txt", "text/plain", bytes)
		).resolves.toEqual(DONE);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "stage_pasted",
			device: "device-1",
			name: "a.txt",
			mime: "text/plain",
			data: "aGk=",
		});
	});

	it("refuses a paste over 8 MiB before sending it", async () => {
		const bytes = new Uint8Array(MAX_PASTE + 1);
		await expect(
			tauriSource().stagePasted("device-1", "big.png", "image/png", bytes)
		).resolves.toEqual({
			status: "error",
			error: { kind: "failed", message: "A paste can be at most 8 MiB" },
		});
		expect(commands.request).not.toHaveBeenCalled();
	});

	it("encodes bytes past one chunk the way atob reads them back", () => {
		const bytes = Uint8Array.from({ length: 70_000 }, (_, i) => (i * 7) % 256);
		const back = Uint8Array.from(atob(toBase64(bytes)), (char) => char.charCodeAt(0));
		expect(back).toEqual(bytes);
	});

	it("sends a setup-token to each device with one result for each", async () => {
		const replies = [DONE, { status: "error", error: { kind: "not_connected", device: "nl" } }];
		commands.request.mockResolvedValue(answered("store_claude_token", replies));
		await expect(
			tauriSource().storeClaudeToken(
				["device-1", "device-2"],
				"account-1",
				"Max",
				"test-not-a-real-token"
			)
		).resolves.toEqual(replies);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "store_claude_token",
			devices: ["device-1", "device-2"],
			origin: "account-1",
			label: "Max",
			token: "test-not-a-real-token",
		});
	});

	it("sends a Cursor API key to each device with one result for each", async () => {
		const replies = [DONE, DONE];
		commands.request.mockResolvedValue(answered("store_cursor_key", replies));
		await expect(
			tauriSource().storeCursorKey(["device-1", "device-2"], "Work", "test-not-a-real-key")
		).resolves.toEqual(replies);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "store_cursor_key",
			devices: ["device-1", "device-2"],
			label: "Work",
			token: "test-not-a-real-key",
		});
	});

	it("sends one inbox change for several agents with one result for each", async () => {
		const replies = [DONE, { status: "error", error: { kind: "not_connected", device: "nl" } }];
		commands.request.mockResolvedValue(answered("set_inbox", replies));
		await expect(
			tauriSource().setInbox(["agent-1", "agent-2"], { change: "snooze", until: 1_800 })
		).resolves.toEqual(replies);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "set_inbox",
			agents: ["agent-1", "agent-2"],
			change: { change: "snooze", until: 1_800 },
		});
	});

	it("sends a GitHub App's key path for the backend to read", async () => {
		const replies = [DONE, DONE];
		commands.request.mockResolvedValue(answered("set_github_app", replies));
		await expect(
			tauriSource().setGithubApp(["device-1", "device-2"], 123456, "/Users/max/app.pem")
		).resolves.toEqual(replies);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "set_github_app",
			devices: ["device-1", "device-2"],
			app_id: 123456,
			key_path: "/Users/max/app.pem",
		});
	});

	it("sends an APNs key's path for the backend to read", async () => {
		const replies = [DONE];
		commands.request.mockResolvedValue(answered("set_apns_key", replies));
		await expect(
			tauriSource().setApnsKey(["device-1"], "ABC123DEFG", "<TEAM_ID>", "/tmp/AuthKey.p8")
		).resolves.toEqual(replies);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "set_apns_key",
			devices: ["device-1"],
			key_id: "ABC123DEFG",
			team_id: "<TEAM_ID>",
			key_path: "/tmp/AuthKey.p8",
		});
	});

	it("uploads each path in turn, each with a progress channel of its own", async () => {
		const failed = { status: "error", error: { kind: "not_connected", device: "nl" } };
		commands.upload.mockResolvedValueOnce(DONE).mockResolvedValueOnce(failed);
		const heard: TransferProgress[] = [];
		await expect(
			tauriSource().upload("device-1", TO_AGENT, ["/a", "/b"], (p) => heard.push(p))
		).resolves.toEqual([DONE, failed]);
		const [first, second, ...more] = FakeChannel.made;
		expect(more).toEqual([]);
		expect(commands.upload.mock.calls).toEqual([
			["device-1", TO_AGENT, "/a", first],
			["device-1", TO_AGENT, "/b", second],
		]);
		first?.onmessage(step("uploading"));
		first?.onmessage(step("done"));
		expect(heard).toEqual([step("uploading"), step("done")]);
	});

	it("uploads a paste as base64 with its own progress channel", async () => {
		commands.uploadPasted.mockResolvedValue(DONE);
		const heard: TransferProgress[] = [];
		const bytes = new TextEncoder().encode("hi");
		await expect(
			tauriSource().uploadPasted("device-1", TO_AGENT, "a.txt", "text/plain", bytes, (p) =>
				heard.push(p)
			)
		).resolves.toEqual(DONE);
		const [channel] = FakeChannel.made;
		expect(commands.uploadPasted).toHaveBeenCalledExactlyOnceWith(
			"device-1",
			TO_AGENT,
			"a.txt",
			"text/plain",
			"aGk=",
			channel
		);
		channel?.onmessage(step("done"));
		expect(heard).toEqual([step("done")]);
	});

	it("downloads with a progress channel and gives where the file landed", async () => {
		const landed = { status: "ok", data: { path: "/Users/max/Downloads/MaxHQ/a.txt" } };
		commands.download.mockResolvedValue(landed);
		const heard: TransferProgress[] = [];
		await expect(
			tauriSource().download("agent-1", "/home/dev/a.txt", (p) => heard.push(p))
		).resolves.toEqual(landed);
		const [channel] = FakeChannel.made;
		expect(commands.download).toHaveBeenCalledExactlyOnceWith(
			"agent-1",
			"/home/dev/a.txt",
			channel
		);
		channel?.onmessage(step("done"));
		expect(heard).toEqual([step("done")]);
	});

	it("fetches media with a progress channel and gives the cached file's URL", async () => {
		const name = `${"a".repeat(64)}.png`;
		commands.media.mockResolvedValue({ status: "ok", data: { kind: "cached", name, size: 4 } });
		const heard: TransferProgress[] = [];
		await expect(
			tauriSource().media("agent-1", "shots/a.png", 1024, false, (p) => heard.push(p))
		).resolves.toEqual({
			status: "ok",
			data: { kind: "cached", url: `maxmedia://localhost/${name}`, size: 4 },
		});
		const [channel] = FakeChannel.made;
		expect(commands.media).toHaveBeenCalledExactlyOnceWith(
			"agent-1",
			"shots/a.png",
			1024,
			false,
			channel
		);
		channel?.onmessage(step("done"));
		expect(heard).toEqual([step("done")]);
	});

	it("gives a file's size when it's past the limit, and keeps the limit to what the command takes", async () => {
		const large = { status: "ok", data: { kind: "too_large", size: 9_000_000_000 } };
		commands.media.mockResolvedValue(large);
		await expect(
			tauriSource().media("agent-1", "/w/a.mp4", 2 ** 40, true, () => {})
		).resolves.toEqual(large);
		expect(commands.media.mock.calls[0]?.[2]).toBe(0xffff_ffff);
		expect(commands.media.mock.calls[0]?.[3]).toBe(true);
	});

	it("passes on a path the device refused", async () => {
		const refused = {
			status: "ok",
			data: {
				kind: "refused",
				outside: true,
				message: "the path is outside the agent's folders",
			},
		};
		commands.media.mockResolvedValue(refused);
		await expect(
			tauriSource().media("agent-1", "/tmp/example/a.md", 1024, false, () => {})
		).resolves.toEqual(refused);
	});

	it("rejects a forward that doesn't close or a localhost that doesn't open", async () => {
		const source = tauriSource();
		await expect(source.closeForward("forward-1")).resolves.toBeUndefined();
		await expect(source.openLocalhost(5173)).resolves.toBeUndefined();
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "close_forward", forward: "forward-1" }],
			[{ kind: "open_localhost", port: 5173 }],
		]);

		const error = { kind: "failed", message: "Another computer opened this forward" };
		commands.request.mockResolvedValue({ status: "error", error });
		await expect(source.closeForward("forward-1")).rejects.toThrow(error.message);
		await expect(source.openLocalhost(5173)).rejects.toHaveProperty("cause", error);
	});
});

describe("tauri source timelines", () => {
	it("makes no channel until a timeline opens", () => {
		tauriSource();
		expect(FakeChannel.made).toHaveLength(0);
	});

	it("opens every timeline over the window's one channel", async () => {
		const source = tauriSource();
		await source.openTimeline("agent-1", () => {});
		await source.openTimeline("agent-2", () => {});
		expect(FakeChannel.made).toHaveLength(1);
		const [channel] = FakeChannel.made;
		expect(commands.openTimeline.mock.calls).toEqual([
			["agent-1", channel],
			["agent-2", channel],
		]);
	});

	it("hands each update to the agent it's for, in order", async () => {
		const source = tauriSource();
		const first: TimelineUpdate[] = [];
		const second: TimelineUpdate[] = [];
		await source.openTimeline("agent-1", (update) => first.push(update));
		await source.openTimeline("agent-2", (update) => second.push(update));
		send(
			{ agent: "agent-1", update: reset(1) },
			{ agent: "agent-2", update: reset(7) },
			{ agent: "agent-1", update: events(2) },
			{ agent: "agent-3", update: reset(1) }
		);
		expect(first).toEqual([reset(1), events(2)]);
		expect(second).toEqual([reset(7)]);
	});

	it("says which updates arrived live", async () => {
		const source = tauriSource();
		const seen: [TimelineUpdate, boolean][] = [];
		await source.openTimeline("agent-1", (update, live) => seen.push([update, live]));
		send(
			{ agent: "agent-1", update: reset(1) },
			{ agent: "agent-1", update: events(2), live: false },
			{ agent: "agent-1", update: events(3) }
		);
		expect(seen).toEqual([
			[reset(1), false],
			[events(2), false],
			[events(3), true],
		]);
	});

	it("skips changes that come before an open's own reset", async () => {
		const source = tauriSource();
		const early: TimelineUpdate[] = [];
		const late: TimelineUpdate[] = [];
		await source.openTimeline("agent-1", (update) => early.push(update));
		send({ agent: "agent-1", update: reset(1) });
		await source.openTimeline("agent-1", (update) => late.push(update));
		send({ agent: "agent-1", update: events(2) }, { agent: "agent-1", update: reset(2) });
		send({ agent: "agent-1", update: events(3) });
		expect(early).toEqual([reset(1), events(2), reset(2), events(3)]);
		expect(late).toEqual([reset(2), events(3)]);
	});

	it("closes once for each open and stops handing that open updates", async () => {
		const source = tauriSource();
		const kept: TimelineUpdate[] = [];
		const dropped: TimelineUpdate[] = [];
		await source.openTimeline("agent-1", (update) => kept.push(update));
		const stop = await source.openTimeline("agent-1", (update) => dropped.push(update));
		stop();
		stop();
		send({ agent: "agent-1", update: reset(1) });
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "close_timeline",
			agent: "agent-1",
		});
		expect(kept).toEqual([reset(1)]);
		expect(dropped).toEqual([]);
	});

	it("rejects an open that fails and stops listening for it", async () => {
		const error = { kind: "not_connected", device: "nl" };
		commands.openTimeline.mockResolvedValue({ status: "error", error });
		const source = tauriSource();
		const seen: TimelineUpdate[] = [];
		const opening = source.openTimeline("agent-1", (update) => seen.push(update));
		await expect(opening).rejects.toThrow("nl isn't connected");
		await expect(opening).rejects.toHaveProperty("cause", error);
		send({ agent: "agent-1", update: reset(1) });
		expect(seen).toEqual([]);
		expect(commands.request).not.toHaveBeenCalled();
	});
});

describe("fleet calls", () => {
	it("reads load for each device and hands back the list", async () => {
		const loads = [{ device: "device-1", load: null, received_at: "2026-10-31T09:00:00Z" }];
		commands.request.mockResolvedValue(answered("read_load", loads));
		await expect(tauriSource().readLoad(["device-1"])).resolves.toEqual(loads);
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "read_load",
			devices: ["device-1"],
		});
	});

	it("unwraps diagnostics and refuses any other answer", async () => {
		const diagnostics = { buckets: [], top: [], containers: [], error: null };
		commands.request.mockResolvedValueOnce(
			answered("read_diagnostics", { status: "diagnostics", ...diagnostics })
		);
		await expect(tauriSource().readDiagnostics("device-1", 3600, 60)).resolves.toEqual({
			status: "ok",
			data: diagnostics,
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "read_diagnostics",
			device: "device-1",
			window_secs: 3600,
			bucket_secs: 60,
		});
		commands.request.mockResolvedValueOnce(answered("read_diagnostics", { status: "done" }));
		await expect(tauriSource().readDiagnostics("device-1", 3600, 60)).resolves.toHaveProperty(
			"status",
			"error"
		);
	});

	it("updates the fleet with a progress channel", async () => {
		const heard: unknown[] = [];
		await expect(
			tauriSource().updateFleet(["device-1"], true, (p) => heard.push(p))
		).resolves.toEqual(OK);
		const [channel, ...more] = FakeChannel.made;
		expect(more).toEqual([]);
		expect(commands.updateFleet).toHaveBeenCalledExactlyOnceWith(["device-1"], true, channel);
		channel?.onmessage({ device: "device-1", status: { status: "waiting" } });
		expect(heard).toEqual([{ device: "device-1", status: { status: "waiting" } }]);
	});

	it("sets a device up with a progress channel and answers host key questions", async () => {
		const heard: unknown[] = [];
		await tauriSource().setupDevice("devbox", [], (p) => heard.push(p));
		const [channel] = FakeChannel.made;
		expect(commands.setupDevice).toHaveBeenCalledExactlyOnceWith("devbox", [], channel);
		await tauriSource().confirmHostKey("setup-0", true);
		expect(commands.request).toHaveBeenCalledWith({
			kind: "confirm_host_key",
			setup: "setup-0",
			ok: true,
		});
	});

	it("lists the hosts a setup can start from", async () => {
		const hosts = [{ host: "devbox", from: "ssh_config", address: null, link: null }];
		commands.request.mockResolvedValue(answered("list_setup_hosts", hosts));
		await expect(tauriSource().listSetupHosts()).resolves.toEqual(hosts);
	});
});

describe("agent tools calls", () => {
	it("lists permissions from one device or every device", async () => {
		commands.request.mockResolvedValue(answered("list_permissions", [PERMISSION]));
		await expect(tauriSource().listPermissions(null)).resolves.toEqual({
			status: "ok",
			data: [PERMISSION],
		});
		await tauriSource().listPermissions("device-1");
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "list_permissions", device: null }],
			[{ kind: "list_permissions", device: "device-1" }],
		]);
	});

	it("lists access keys with the command that uses each", async () => {
		const path = "/home/dev/.local/share/maxhq/keys/ci-bot.key";
		const key: AccessKey = {
			id: "key-1",
			device: "device-1",
			name: "ci-bot",
			path,
			permission: "permission-1",
			command: { command: "/home/dev/.local/bin/maxd", args: ["mcp", "--key-file", path] },
		};
		commands.request.mockResolvedValue(answered("list_keys", [key]));
		await expect(tauriSource().listKeys("device-1")).resolves.toEqual({
			status: "ok",
			data: [key],
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "list_keys",
			device: "device-1",
		});
	});

	it("lists a device's links to the others", async () => {
		const link = { peer: "device-2", name: "devbox", state: "linked", note: null } as const;
		commands.request.mockResolvedValue(answered("list_device_links", [link]));
		await expect(tauriSource().listDeviceLinks("device-1")).resolves.toEqual({
			status: "ok",
			data: [link],
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "list_device_links",
			device: "device-1",
		});
	});

	it("starts a phone pairing through one device for the others", async () => {
		const pairing = {
			pair: "pair-1",
			device: "device-1",
			qr: { size: 21, path: "M0 0h7v7h-7z" },
			expires_at: 0,
		};
		commands.request.mockResolvedValue(answered("begin_phone_pair", pairing));
		await expect(tauriSource().beginPhonePair("device-1", ["device-2"])).resolves.toEqual({
			status: "ok",
			data: pairing,
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "begin_phone_pair",
			through: "device-1",
			devices: ["device-2"],
		});
	});

	it("reads the defaults and rejects when they can't be read", async () => {
		const defaults = { limits: NO_LIMITS, depth: 3 };
		commands.request.mockResolvedValueOnce(answered("agent_defaults", defaults));
		await expect(tauriSource().agentDefaults()).resolves.toEqual(defaults);
		commands.request.mockResolvedValueOnce({
			status: "error",
			error: { kind: "failed", message: "The database is locked" },
		});
		await expect(tauriSource().agentDefaults()).rejects.toThrow("The database is locked");
	});
});

describe("scheduled task calls", () => {
	it("lists tasks from one device or every device", async () => {
		commands.request.mockResolvedValue(answered("list_tasks", [TASK]));
		await expect(tauriSource().listTasks(null)).resolves.toEqual({
			status: "ok",
			data: [TASK],
		});
		await tauriSource().listTasks("device-1");
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "list_tasks", device: null }],
			[{ kind: "list_tasks", device: "device-1" }],
		]);
	});

	it("reads a device's Funnel", async () => {
		const funnel = { state: "on", url: "https://devbox.example.ts.net" } as const;
		commands.request.mockResolvedValue(answered("funnel_state", funnel));
		await expect(tauriSource().funnelState("device-1")).resolves.toEqual({
			status: "ok",
			data: funnel,
		});
		expect(commands.request).toHaveBeenCalledExactlyOnceWith({
			kind: "funnel_state",
			device: "device-1",
		});
	});

	it("lists a task's runs and deliveries", async () => {
		const run: Run = {
			task: "task-1",
			cause: "hook",
			started: 1,
			ended: null,
			outcome: "running",
			agent: "agent-1",
			delivery: "delivery-1",
			error: null,
		};
		const delivery: TaskDelivery = {
			id: "delivery-1",
			task: "task-1",
			received: 1,
			method: "POST",
			content_type: "application/json",
			body_bytes: 2,
			outcome: "accepted",
			signature_verified: true,
			missing_fields: [],
			error: null,
		};
		commands.request.mockImplementation(async (request: Request) =>
			request.kind === "list_runs"
				? answered(request.kind, [run])
				: answered(request.kind, [delivery])
		);
		const source = tauriSource();
		await expect(source.listRuns("task-1")).resolves.toEqual({ status: "ok", data: [run] });
		await expect(source.listDeliveries("task-1")).resolves.toEqual({
			status: "ok",
			data: [delivery],
		});
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "list_runs", task: "task-1" }],
			[{ kind: "list_deliveries", task: "task-1" }],
		]);
	});

	it("hands a new webhook URL to the caller each time it comes", async () => {
		const url = "https://devbox.example.ts.net/hooks/test-not-a-real-token";
		commands.request.mockResolvedValue(
			answered("rotate_hook_token", { status: "hook_url", url })
		);
		const source = tauriSource();
		await expect(source.rotateHookToken("task-1")).resolves.toEqual({
			status: "ok",
			data: { status: "hook_url", url },
		});
		commands.request.mockResolvedValue(answered("rotate_hook_token", { status: "done" }));
		await expect(source.rotateHookToken("task-1")).resolves.toEqual(DONE);
	});

	it("sends each test host request to its device and reads the screenshot", async () => {
		const screen = {
			status: "lease_screen",
			image: { mime: "image/png", data: "aGk=" },
			at: 1,
		};
		commands.request.mockImplementation(async (request: Request) =>
			answered(
				request.kind,
				request.kind === "read_lease_screen" ? screen : { status: "done" }
			)
		);
		const source = tauriSource();
		await expect(source.checkTestHosts("device-1")).resolves.toEqual(DONE);
		await expect(source.buildTestHost("device-1", "host-1")).resolves.toEqual(DONE);
		await expect(source.resetTestHost("device-1", "host-1")).resolves.toEqual(DONE);
		await expect(source.endLease("device-1", "lease-1")).resolves.toEqual(DONE);
		expect(leaseScreenOf(await source.readLeaseScreen("device-1", "lease-1"))).toEqual({
			src: "data:image/png;base64,aGk=",
			at: 1,
		});
		expect(commands.request.mock.calls).toEqual([
			[{ kind: "check_test_hosts", device: "device-1" }],
			[{ kind: "build_test_host", device: "device-1", host: "host-1" }],
			[{ kind: "reset_test_host", device: "device-1", host: "host-1" }],
			[{ kind: "end_lease", device: "device-1", lease: "lease-1" }],
			[{ kind: "read_lease_screen", device: "device-1", lease: "lease-1" }],
		]);
	});
});
