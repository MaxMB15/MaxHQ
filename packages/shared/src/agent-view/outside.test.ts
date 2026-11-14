// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { beforeEach, describe, expect, it } from "vitest";

import type { Agent } from "../bindings";
import { agent, MIN, NOW } from "../test/build";
import { TAKE_OVER_SINCE } from "../bindings";
import { clearKept } from "./kept";
import {
	canTakeOver,
	followLead,
	MANUAL_NOTE,
	movedNote,
	movedTo,
	outsideNote,
	outsideOffer,
	type NotePart,
	rootLead,
	takeOverQuestion,
	takingOverNote,
	WAIT_NOTE,
} from "./outside";

const text = (parts: readonly NotePart[]) => parts.map((part) => part.text).join("");

describe("outsideNote", () => {
	it("reads only while a terminal runs the session, and names its command", () => {
		const note = outsideNote(agent({ id: "a1" }), "devbox");
		expect(note.ended).toBe(false);
		expect(note.title).toBe("Running in a terminal on devbox. Read only here.");
		expect(note.body).toContainEqual({ text: "claude", code: true });
		expect(text(note.body)).toBe(
			"Started with claude in a terminal. Answer it and send messages in that terminal. This view follows along."
		);
	});

	it("leaves out the place when it isn't known", () => {
		expect(outsideNote(agent({ id: "a1" }), null).title).toBe(
			"Running in a terminal. Read only here."
		);
	});

	it("names no command for an agent MaxHQ knows none for", () => {
		const note = outsideNote(agent({ id: "a1", vendor: "acp" }), "devbox");
		expect(note.body.some((part) => part.code)).toBe(false);
		expect(text(note.body)).toMatch(/^Started in a terminal\./);
	});

	it("offers to continue once the terminal let go", () => {
		const down = agent({
			id: "a1",
			vendor: "codex",
			link: { state: "down", reason: "exited", since: null },
		});
		const note = outsideNote(down, "devbox");
		expect(note.ended).toBe(true);
		expect(note.title).toBe("The terminal session ended. You can continue it here.");
		expect(text(note.body)).toBe(
			"Codex keeps the history and context. The terminal is no longer attached."
		);
		expect(note.wait).toBe(WAIT_NOTE);
	});

	describe("while the device can't be reached", () => {
		const unreachable = (last_seen: number | null) => ({
			link: { state: "unreachable", reason: "Tailscale is off", last_seen } as const,
			now: NOW,
		});

		it("says so, and when the device last answered", () => {
			const note = outsideNote(agent({ id: "a1" }), "devbox", unreachable(NOW - 12 * MIN));
			expect(note.ended).toBe(false);
			expect(note.title).toBe("Can't reach devbox right now.");
			expect(text(note.body)).toBe(
				"It last answered at 14:36, 12 min ago. The session may still run in the terminal there. This view shows what MaxHQ saw last."
			);
			expect(text(note.wait)).toBe(
				"Turns on once MaxHQ reaches devbox again and the session in the terminal has ended."
			);
		});

		it("leaves out when it last answered if the view doesn't know", () => {
			const note = outsideNote(agent({ id: "a1" }), "devbox", unreachable(null));
			expect(text(note.body)).toMatch(/^The session may still run/);
		});

		it("says so when the agent can't be seen, even without the link", () => {
			const hidden = agent({ id: "a1", activity: { state: "not_accessible" } });
			expect(outsideNote(hidden, null).title).toBe("Can't reach the device right now.");
		});

		it("keeps Continue off even when the session had ended", () => {
			const down = agent({
				id: "a1",
				link: { state: "down", reason: "exited", since: null },
			});
			expect(outsideNote(down, "devbox", unreachable(null)).ended).toBe(false);
		});

		it("follows along again once the device answers", () => {
			const note = outsideNote(agent({ id: "a1" }), "devbox", {
				link: { state: "connected", since: NOW - MIN },
				now: NOW,
			});
			expect(note.title).toBe("Running in a terminal on devbox. Read only here.");
		});
	});

	describe("after an error", () => {
		const errored = (over: Partial<Agent> = {}) =>
			agent({
				id: "a1",
				activity: { state: "error", reason: "exited with status 1" },
				...over,
			});

		it("gives the reason once the session ended", () => {
			const note = outsideNote(
				errored({ link: { state: "down", reason: "exited", since: null } }),
				"devbox"
			);
			expect(note.ended).toBe(true);
			expect(note.title).toBe(
				"The terminal session stopped with an error. You can continue it here."
			);
			expect(text(note.body)).toBe(
				"Exited with status 1. Claude keeps the history and context. The terminal is no longer attached."
			);
		});

		it("gives the reason before the terminal lets go", () => {
			const note = outsideNote(errored(), "devbox");
			expect(note.title).toBe(
				"Stopped with an error in a terminal on devbox. Read only here."
			);
			expect(text(note.body)).toBe(
				"Exited with status 1. Started with claude in a terminal."
			);
		});
	});
});

describe("canTakeOver", () => {
	const live = agent({ id: "a1" });
	const up = { state: "connected", since: NOW } as const;

	it("offers a live Claude or Codex terminal session on a maxd that knows how", () => {
		expect(canTakeOver(live, TAKE_OVER_SINCE, up)).toBe(true);
		expect(canTakeOver(agent({ id: "a1", vendor: "codex" }), TAKE_OVER_SINCE, null)).toBe(true);
	});

	it("hides the button on an older maxd or an unknown one", () => {
		expect(canTakeOver(live, TAKE_OVER_SINCE - 1, up)).toBe(false);
		expect(canTakeOver(live, null, up)).toBe(false);
	});

	it("hides it for Cursor and other ACP agents", () => {
		expect(canTakeOver(agent({ id: "a1", vendor: "cursor" }), TAKE_OVER_SINCE, up)).toBe(false);
		expect(canTakeOver(agent({ id: "a1", vendor: "acp" }), TAKE_OVER_SINCE, up)).toBe(false);
	});

	it("hides it once the terminal let go, or while the device can't be reached", () => {
		const down = agent({ id: "a1", link: { state: "down", reason: "exited", since: null } });
		expect(canTakeOver(down, TAKE_OVER_SINCE, up)).toBe(false);
		const away = { state: "unreachable", reason: "auth failed", last_seen: NOW - MIN } as const;
		expect(canTakeOver(live, TAKE_OVER_SINCE, away)).toBe(false);
	});

	it("hides it for a session MaxHQ already holds", () => {
		expect(canTakeOver(agent({ id: "a1", control: "held" }), TAKE_OVER_SINCE, up)).toBe(false);
	});
});

describe("rootLead", () => {
	const lead = agent({ id: "lead" });
	const sub = agent({ id: "sub", parent: "lead", pid: null });
	const nested = agent({ id: "nested", parent: "sub", pid: null });
	const agents = new Map([lead, sub, nested].map((a) => [a.id, a]));

	it("is the agent itself for a lead", () => {
		expect(rootLead(agents, lead)).toBe(lead);
	});

	it("walks up to the top, however deep", () => {
		expect(rootLead(agents, sub)).toBe(lead);
		expect(rootLead(agents, nested)).toBe(lead);
	});

	it("is null when the view lacks a parent on the way", () => {
		expect(rootLead(new Map([["nested", nested]]), nested)).toBeNull();
		const loop = agent({ id: "loop", parent: "loop" });
		expect(rootLead(new Map([["loop", loop]]), loop)).toBeNull();
	});
});

describe("outsideOffer", () => {
	const up = { state: "connected", since: NOW } as const;
	const away = { state: "unreachable", reason: "auth failed", last_seen: NOW - MIN } as const;
	const offer = (
		sub: Agent,
		lead: Agent | null,
		protocol: number | null = TAKE_OVER_SINCE,
		link: Parameters<typeof outsideOffer>[4] = up
	) => outsideOffer(sub, lead, outsideNote(sub, "devbox"), protocol, link, "devbox");
	const lead = (over: Partial<Agent> = {}) => agent({ id: "lead", vendor: "codex", ...over });
	const sub = (over: Partial<Agent> = {}) =>
		agent({ id: "sub", vendor: "codex", parent: "lead", pid: null, ...over });
	const why = (result: ReturnType<typeof outsideOffer>) =>
		result.kind === "off" && result.button === "take_over" ? text(result.why) : null;

	it("keeps a lead's own offer: take over, wait, continue or wait for /exit", () => {
		expect(offer(lead(), lead())).toEqual({ kind: "take_over" });
		expect(offer(lead({ taking_over: true }), null)).toEqual({ kind: "taking" });
		const down = lead({ link: { state: "down", reason: "exited", since: null } });
		expect(offer(down, down)).toEqual({ kind: "continue" });
		expect(offer(lead(), lead(), TAKE_OVER_SINCE - 1)).toEqual({
			kind: "off",
			button: "continue",
			why: WAIT_NOTE,
		});
	});

	it("offers to take over the lead of a Codex or Claude sub-agent", () => {
		expect(offer(sub(), lead())).toEqual({ kind: "take_over" });
		expect(
			offer(sub({ vendor: "claude" }), lead({ vendor: "claude" }), TAKE_OVER_SINCE, null)
		).toEqual({ kind: "take_over" });
	});

	it("offers it for a nested sub-agent too, by its lead at the top", () => {
		const nested = sub({ id: "nested", parent: "sub" });
		expect(offer(nested, lead())).toEqual({ kind: "take_over" });
	});

	it("waits with the lead while it takes over, and points to it once it's held", () => {
		expect(offer(sub(), lead({ taking_over: true }))).toEqual({ kind: "taking" });
		const held = lead({ control: "held", pid: null });
		expect(offer(sub(), held)).toEqual({ kind: "moved", lead: held });
	});

	it("says why it can't, and never to quit the CLI", () => {
		const reasons = [
			why(offer(sub(), null)),
			why(offer(sub(), lead(), TAKE_OVER_SINCE, away)),
			why(offer(sub(), lead({ activity: { state: "not_accessible" } }))),
			why(offer(sub(), lead({ link: { state: "down", reason: "exited", since: null } }))),
			why(offer(sub({ vendor: "cursor" }), lead({ vendor: "cursor" }))),
			why(offer(sub(), lead(), TAKE_OVER_SINCE - 1)),
			why(offer(sub(), lead(), null)),
		];
		expect(reasons).toEqual([
			"MaxHQ can't see the session this sub-agent runs in, so it can't take it over.",
			"Turns on once MaxHQ reaches devbox again.",
			"Turns on once MaxHQ reaches devbox again.",
			"The session this sub-agent ran in left the terminal. Continue it here from its lead.",
			"MaxHQ can take over only Claude and Codex sessions from a terminal.",
			"Taking over needs a newer maxd on devbox.",
			"Taking over needs a newer maxd on devbox.",
		]);
		for (const reason of reasons) expect(reason).not.toMatch(/exit|quit/i);
	});

	it("names the device when the place isn't known", () => {
		const result = outsideOffer(sub(), lead(), outsideNote(sub(), null), null, up, null);
		expect(why(result)).toBe("Taking over needs a newer maxd on the device.");
	});
});

describe("movedTo", () => {
	beforeEach(clearKept);
	const lead = agent({ id: "lead", vendor: "codex", control: "held", pid: null });

	it("points a gone sub-agent's view to its lead once MaxHQ holds it", () => {
		followLead("sub", "lead");
		expect(movedTo(new Map([["lead", lead]]), "sub")).toBe(lead);
	});

	it("points nowhere while the lead still runs in the terminal, or isn't in view", () => {
		followLead("sub", "lead");
		const observed = agent({ id: "lead" });
		expect(movedTo(new Map([["lead", observed]]), "sub")).toBeNull();
		expect(movedTo(new Map(), "sub")).toBeNull();
	});

	it("knows only the sub-agents whose view showed, and never a lead's own", () => {
		followLead("lead", "lead");
		expect(movedTo(new Map([["lead", lead]]), "lead")).toBeNull();
		expect(movedTo(new Map([["lead", lead]]), "other")).toBeNull();
	});

	it("says where the session went, and that its lead can pick it up again", () => {
		expect(movedNote(lead)).toEqual({
			title: "This sub-agent's session continues in MaxHQ.",
			body: "The sub-agent stopped when its session left the terminal. Codex keeps its history, so the session can pick it up again, and then it shows here.",
		});
	});
});

describe("takeOverQuestion", () => {
	it("asks once while idle, naming what stops and where", () => {
		expect(takeOverQuestion(agent({ id: "a1" }), "devbox")).toEqual({
			title: "Take over this session?",
			detail: "This stops claude in the terminal on devbox. The terminal window stays open, but the session leaves it and continues here.",
			action: "Take over",
		});
	});

	it("offers to wait for the turn, or to stop now, while a turn runs", () => {
		const question = takeOverQuestion(
			agent({ id: "a1", vendor: "codex", activity: { state: "busy" } }),
			"devbox"
		);
		expect(question.action).toBe("Take over after this turn");
		expect(question.alternative).toBe("Stop now");
		expect(question.detail).toMatch(/^This stops codex in the terminal on devbox\./);
		expect(question.detail).toMatch(/Stop now cuts off the turn in progress\.$/);
	});

	it("says a lead's sub-agents stop with it, and can be picked up again", () => {
		const lead = agent({ id: "a1", vendor: "codex" });
		expect(takeOverQuestion(lead, "devbox", lead, true).detail).toBe(
			"This stops codex in the terminal on devbox. The terminal window stays open, but the session leaves it and continues here. Sub-agents still working stop with it. Codex keeps their history, so the session can pick them up again once it runs here."
		);
	});

	it("asks from a sub-agent's view about the session it runs in", () => {
		const lead = agent({ id: "lead", vendor: "codex" });
		const sub = agent({ id: "sub", vendor: "codex", parent: "lead", pid: null });
		expect(takeOverQuestion(sub, "devbox", lead)).toEqual({
			title: "Take over this sub-agent's session?",
			detail: "This sub-agent runs inside a Codex session in the terminal on devbox. Taking over stops codex there and continues that session here. Sub-agents still working stop with it. Codex keeps their history, so the session can pick them up again once it runs here.",
			action: "Take over",
		});
	});

	it("waits for the lead's turn, not the sub-agent's", () => {
		const lead = agent({ id: "lead", activity: { state: "busy" } });
		const sub = agent({ id: "sub", parent: "lead", pid: null });
		const question = takeOverQuestion(sub, "devbox", lead);
		expect(question.detail).toMatch(/^This sub-agent runs inside a Claude session/);
		expect(question.action).toBe("Take over after this turn");
		expect(question.alternative).toBe("Stop now");
		const idle = takeOverQuestion(
			agent({ id: "sub", parent: "lead", activity: { state: "busy" } }),
			"devbox",
			agent({ id: "lead" })
		);
		expect(idle.alternative).toBeUndefined();
	});
});

describe("takingOverNote", () => {
	it("says MaxHQ waits for the turn, with or without the place", () => {
		expect(takingOverNote(agent({ id: "a1" }), "devbox")).toBe(
			"MaxHQ stops claude in the terminal on devbox once this turn ends, then continues the session here."
		);
		expect(takingOverNote(agent({ id: "a1" }), null)).toBe(
			"MaxHQ stops claude in the terminal once this turn ends, then continues the session here."
		);
	});

	it("speaks of the session's turn in a sub-agent's view", () => {
		const lead = agent({ id: "lead", vendor: "codex" });
		const sub = agent({ id: "sub", vendor: "codex", parent: "lead" });
		expect(takingOverNote(sub, "devbox", lead)).toBe(
			"MaxHQ stops codex in the terminal on devbox once the session's turn ends, then continues the session here."
		);
	});

	it("keeps /exit as the manual way out", () => {
		expect(text(MANUAL_NOTE)).toBe(
			"Or quit it in the terminal yourself with /exit. A session can only run in one place at a time."
		);
	});
});
