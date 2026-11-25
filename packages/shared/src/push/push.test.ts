// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { t } from "../i18n";

import type { ActionResult } from "../data/source";
import { SYNC_GRACE_MS } from "../status/reach";
import { agent, attention, entities, link, NOW, workspace } from "../test/build";
import { applySnapshot } from "../view/replica";
import { FCM_SINCE } from "../bindings";
import {
	answerFromButton,
	awaitingPushUpdate,
	DEFAULT_KINDS,
	denyAnswer,
	denyFromButton,
	handledNotifications,
	PUSH_KINDS,
	pushEnvironment,
	pushKinds,
	pushTarget,
	pushToken,
	pushUpdateNote,
	readKinds,
	switchKind,
	takesPushes,
	targetPayload,
} from "./push";

const AGENT = "00000000-0000-4000-8000-000000000001";
const ATTENTION = "00000000-0000-4000-8000-000000000003";
const CLAUDE = [
	{ id: "allow", label: "Allow" },
	{ id: "allow_session", label: "Allow for this session" },
	{ id: "deny", label: "Deny" },
];
const CODEX = [
	{ id: "accept", label: "Accept" },
	{ id: "decline", label: "Decline" },
	{ id: "cancel", label: "Cancel" },
];

describe("kinds", () => {
	it("starts with every kind on and keeps settings order", () => {
		expect(readKinds(null)).toEqual(["needs_input", "error", "finished", "limit"]);
		const off = switchKind(DEFAULT_KINDS, "error", false);
		expect(off).toEqual(["needs_input", "finished", "limit"]);
		expect(switchKind(off, "error", true)).toEqual(DEFAULT_KINDS);
		expect(readKinds(JSON.stringify(["limit", "needs_input", "nonsense"]))).toEqual([
			"needs_input",
			"limit",
		]);
		expect(readKinds("[]")).toEqual([]);
	});

	it("falls back to every kind when what was kept doesn't read", () => {
		expect(readKinds("{")).toEqual(DEFAULT_KINDS);
		expect(readKinds('"needs_input"')).toEqual(DEFAULT_KINDS);
	});

	it("tells an Android phone that Deny opens MaxHQ", () => {
		const prompt = (kinds: typeof PUSH_KINDS) =>
			kinds.find((each) => each.kind === "needs_input");
		const plain =
			"A permission, a question or a secret. Open it to answer. Deny also works from the lock screen.";
		expect(prompt(PUSH_KINDS)?.detail).toBe(t(plain));
		expect(prompt(pushKinds())?.detail).toBe(t(plain));
		expect(prompt(pushKinds("android"))?.detail).toBe(
			"A permission, a question or a secret. Open it to answer. Deny also works from the notification and opens MaxHQ once the phone is unlocked."
		);
		expect(pushKinds("android").map((each) => each.kind)).toEqual(DEFAULT_KINDS);
	});

	it("names the host from the build", () => {
		expect(pushEnvironment("development")).toBe("sandbox");
		expect(pushEnvironment("production")).toBe("production");
		expect(pushEnvironment(undefined)).toBe("production");
		expect(
			pushToken("test-not-a-real-token", "com.example.maxhq", "sandbox", ["error"])
		).toEqual({
			token: "test-not-a-real-token",
			topic: "com.example.maxhq",
			environment: "sandbox",
			kinds: ["error"],
			service: "apns",
		});
	});

	it("says an Android phone's token is FCM's", () => {
		expect(
			pushToken("test-not-a-real:token", "com.example.maxhq", "production", [], "fcm")
		).toEqual({
			token: "test-not-a-real:token",
			topic: "com.example.maxhq",
			environment: "production",
			kinds: [],
			service: "fcm",
		});
	});
});

describe("pushes through FCM", () => {
	const view = applySnapshot({
		seq: 1,
		entities: entities({
			links: [
				link({ id: "old", device: "dev-old", protocol: FCM_SINCE - 1 }),
				link({ id: "new", device: "dev-new", protocol: FCM_SINCE }),
				link({ id: "unknown", device: "dev-unknown" }),
				link({ id: "older", device: "dev-older", protocol: 20 }),
			],
		}),
	});

	it("reach only a maxd that knows FCM, or one not heard from yet", () => {
		expect(takesPushes(view, "dev-old", "fcm")).toBe(false);
		expect(takesPushes(view, "dev-new", "fcm")).toBe(true);
		expect(takesPushes(view, "dev-unknown", "fcm")).toBe(true);
		expect(takesPushes(view, "dev-old", "apns")).toBe(true);
	});

	it("name the devices to update, and only for FCM", () => {
		expect(awaitingPushUpdate(view, "fcm")).toEqual(["old", "older"]);
		expect(awaitingPushUpdate(view, "apns")).toEqual([]);
		expect(pushUpdateNote(view, "fcm")).toBe(
			"Update maxd on old and older to get their pushes on this phone."
		);
		expect(pushUpdateNote(view, "apns")).toBeNull();
		const one = applySnapshot({
			seq: 1,
			entities: entities({ links: [link({ id: "old", protocol: FCM_SINCE - 1 })] }),
		});
		expect(pushUpdateNote(one, "fcm")).toBe(
			"Update maxd on old to get its pushes on this phone."
		);
	});
});

describe("pushTarget", () => {
	const aps = { alert: { title: "MaxHQ", body: "An agent needs you" }, "thread-id": AGENT };

	it("reads what the extension opened first", () => {
		const target = pushTarget({
			aps,
			e: ATTENTION,
			m: { agent: AGENT, attention: ATTENTION, options: [...CLAUDE, { id: 4 }] },
		});
		expect(target).toEqual({ agent: AGENT, attention: ATTENTION, options: CLAUDE });
	});

	it("falls back to what maxd sent in the clear", () => {
		expect(pushTarget({ aps, e: ATTENTION, c: "sealed" })).toEqual({
			agent: AGENT,
			attention: ATTENTION,
			options: [],
		});
		expect(pushTarget({ aps })).toEqual({ agent: AGENT, attention: null, options: [] });
	});

	it("reads an Android push, whose agent comes under t", () => {
		const clear = { e: ATTENTION, t: AGENT, k: "needs_input", a: "permission", n: "note" };
		expect(pushTarget(clear)).toEqual({ agent: AGENT, attention: ATTENTION, options: [] });
		expect(
			pushTarget({ ...clear, m: { agent: AGENT, attention: ATTENTION, options: CLAUDE } })
		).toEqual({ agent: AGENT, attention: ATTENTION, options: CLAUDE });
		expect(pushTarget({ ...clear, t: "" })).toBeNull();
	});

	it("ignores a push that isn't MaxHQ's", () => {
		expect(pushTarget(null)).toBeNull();
		expect(pushTarget({ aps: { alert: "hi" } })).toBeNull();
	});

	it("reads back the note that stands in for a button", () => {
		const target = { agent: AGENT, attention: ATTENTION, options: [] };
		expect(pushTarget(targetPayload(target))).toEqual(target);
	});
});

describe("denyAnswer", () => {
	it("reads the no from the choices a push carries", () => {
		const options = [
			{ id: "a1", label: "Allow", kind: "allow_always" },
			{ id: "a2", label: "Reject", kind: "reject_once" },
		];
		const target = pushTarget({
			aps: { "thread-id": AGENT },
			m: { agent: AGENT, attention: ATTENTION, options },
		});
		expect(target?.options).toEqual(options);
		expect(denyAnswer(target?.options ?? [])).toEqual({ type: "option", id: "a2" });
	});

	it("picks the no, whatever the agent calls it", () => {
		expect(denyAnswer(CLAUDE)).toEqual({ type: "option", id: "deny" });
		expect(denyAnswer(CODEX)).toEqual({ type: "option", id: "decline" });
		expect(denyAnswer([{ id: "ok", label: "OK" }])).toBeNull();
		expect(denyAnswer([])).toBeNull();
	});
});

describe("answerFromButton", () => {
	const DONE: ActionResult = { status: "ok", data: { status: "done" } };
	const DOWN: ActionResult = {
		status: "error",
		error: { kind: "not_connected", device: "devbox" },
	};
	const NOT_IN: ActionResult = {
		status: "error",
		error: { kind: "not_found", what: "This request" },
	};
	const sequence = (...results: ActionResult[]) => {
		let calls = 0;
		const send = async () => results[Math.min(calls++, results.length - 1)] as ActionResult;
		return { send, calls: () => calls };
	};
	const noWait = async () => undefined;

	it("tries again while the link comes up and the view comes in", async () => {
		const { send, calls } = sequence(DOWN, NOT_IN, DONE);
		expect(await answerFromButton(send, 20_000, noWait)).toBe("sent");
		expect(calls()).toBe(3);
	});

	it("counts another answer coming first as sent", async () => {
		const { send } = sequence({
			status: "ok",
			data: { status: "refused", reason: "Someone else answered first" },
		});
		expect(await answerFromButton(send, 20_000, noWait)).toBe("sent");
	});

	it("gives up once its patience runs out, or on any other failure", async () => {
		const down = sequence(DOWN);
		expect(await answerFromButton(down.send, 3_000, noWait)).toBe("unreached");
		expect(down.calls()).toBe(4);
		const failed = sequence({ status: "error", error: { kind: "failed", message: "no" } });
		expect(await answerFromButton(failed.send, 3_000, noWait)).toBe("unreached");
		expect(await answerFromButton(() => Promise.reject(new Error("gone")))).toBe("unreached");
	});
});

describe("denyFromButton", () => {
	const DONE: ActionResult = { status: "ok", data: { status: "done" } };
	const noWait = async () => undefined;
	const target = { agent: AGENT, attention: ATTENTION, options: [] };

	it("answers with the choices the extension opened", async () => {
		const sent: unknown[] = [];
		const answer = async (...args: unknown[]) => (sent.push(args), DONE);
		const opened = { ...target, options: CODEX };
		expect(await denyFromButton(opened, () => undefined, answer, 0, noWait)).toBe("sent");
		expect(sent).toEqual([[ATTENTION, { type: "option", id: "decline" }]]);
	});

	it("waits for the view to bring the choices in", async () => {
		let looks = 0;
		const inView = () => (++looks < 3 ? undefined : CLAUDE);
		const sent: unknown[] = [];
		const answer = async (...args: unknown[]) => (sent.push(args), DONE);
		expect(await denyFromButton(target, inView, answer, 20_000, noWait)).toBe("sent");
		expect(looks).toBe(3);
		expect(sent).toEqual([[ATTENTION, { type: "option", id: "deny" }]]);
	});

	it("doesn't answer a push without a request or a fitting choice", async () => {
		const answer = async () => DONE;
		const none = { ...target, attention: null };
		expect(await denyFromButton(none, () => CLAUDE, answer, 0, noWait)).toBe("unreached");
		const ok = () => [{ id: "ok", label: "OK" }];
		expect(await denyFromButton(target, ok, answer, 0, noWait)).toBe("unreached");
		expect(await denyFromButton(target, () => undefined, answer, 2_000, noWait)).toBe(
			"unreached"
		);
	});

	it("leaves a request only its card may answer at once", async () => {
		let looks = 0;
		const inView = () => (looks++, []);
		const answer = async () => DONE;
		expect(await denyFromButton(target, inView, answer, 20_000, noWait)).toBe("unreached");
		expect(looks).toBe(1);
	});
});

describe("handledNotifications", () => {
	const view = (since: number) =>
		applySnapshot({
			seq: 1,
			entities: entities({
				agents: [agent({ id: "a1" }), agent({ id: "a2" })],
				workspaces: [workspace({ id: "w1" })],
				links: [
					link({ id: "mac", device: "dev-mac", state: { state: "connected", since } }),
				],
				attention: [attention({ id: "e-open", agent: "a2" })],
			}),
		});
	const note = (id: string, agent: string, attention: string | null) => ({
		id,
		target: { agent, attention, options: [] },
	});
	const presented = [
		note("n1", "a1", null),
		note("n2", "a2", "e-open"),
		note("n3", "a2", "e-answered"),
		note("n4", "a2", null),
		{ id: "n5", target: null },
	];

	it("clears the agent on screen, and prompts answered since", () => {
		expect(handledNotifications(presented, view(NOW - SYNC_GRACE_MS), "a1", NOW)).toEqual([
			"n1",
			"n3",
		]);
		expect(handledNotifications(presented, view(NOW - SYNC_GRACE_MS), null, NOW)).toEqual([
			"n3",
		]);
	});

	it("keeps prompts until the agent's device has synced", () => {
		expect(handledNotifications(presented, view(NOW - 1), null, NOW)).toEqual([]);
		const gone = [note("n6", "a9", "e-answered")];
		expect(handledNotifications(gone, view(NOW - SYNC_GRACE_MS), null, NOW)).toEqual([]);
	});
});
