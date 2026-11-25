// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { CLOUD_SERVERS_SINCE, type Entity, type ServerCredits } from "../bindings";
import {
	DEVBOX_OPENROUTER_SERVER,
	MAC_SERVER,
	OLLAMA_SERVER,
	OPENROUTER_SERVER,
} from "../data/mock-local-models";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	adminKeyWarning,
	creditsShort,
	creditsView,
	offersAdminKey,
	removeAdminKeyAction,
	resetsText,
	serverCreditRows,
	setAdminKeyAction,
} from "./credits";

const MIN = 60_000;

/** The sample view, with its entities changed by `edit`. */
const load = async (edit: (entities: Entity[]) => Entity[] = (e) => e): Promise<ViewState> => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	return applySnapshot({ ...snapshot, entities: edit(snapshot.entities) });
};

/** The sample view with `device`'s link speaking `protocol`. */
const speaking = (device: string, protocol: number) =>
	load((entities) =>
		entities.map((e) =>
			e.entity === "device_link" && e.device === device ? { ...e, protocol } : e
		)
	);

/** A key's figures as OpenRouter reports them, read a minute before `NOW`. */
const read = (figures: Partial<ServerCredits> = {}): ServerCredits => ({
	at: NOW - MIN,
	used_cents: 258,
	...figures,
});

describe("a server's credits", () => {
	it("shows what's left of a key's limit, when it resets and today's spending", () => {
		const shown = creditsView(
			read({ today_cents: 31, limit_cents: 1_000, remaining_cents: 742, resets: "monthly" }),
			NOW
		);
		expect(shown.bars).toHaveLength(1);
		expect(shown.bars[0]!.text).toBe("$7.42 left of $10.00");
		expect(shown.bars[0]!.window).toMatchObject({
			label: "This key",
			usedUp: false,
			spend: { used_cents: 258, limit_cents: 1_000 },
		});
		expect(shown.bars[0]!.window.percent).toBeCloseTo(25.8);
		expect(shown.status).toBe("Resets monthly · $0.31 today");
		expect(shown.line).toBe("$7.42 left of $10.00 · Resets monthly · $0.31 today");
		expect(shown.short).toBe("$7.42 left");
		expect(shown.stale).toBe(false);
		expect(shown.title).toBe("Checked 1 min ago.");
	});

	it("works out what's left without OpenRouter's figure, never below nothing", () => {
		expect(creditsView(read({ limit_cents: 1_000 }), NOW).short).toBe("$7.42 left");
		const over = creditsView(read({ used_cents: 1_250, limit_cents: 1_000 }), NOW);
		expect(over.bars[0]!.text).toBe("$0.00 left of $10.00");
		expect(over.bars[0]!.window).toMatchObject({ percent: 100, usedUp: true });
		expect(over.status).toBe("");
	});

	it("shows what a key without a limit spent, and today's spending", () => {
		const shown = creditsView(read({ today_cents: 31 }), NOW);
		expect(shown.bars[0]!.text).toBe("$2.58 spent");
		expect(shown.bars[0]!.window).toMatchObject({ label: "This key", percent: null });
		expect(shown.status).toBe("$0.31 today");
		expect(shown.line).toBe("$2.58 spent · $0.31 today");
		expect(shown.short).toBe("$2.58 spent");
		expect(creditsView(read(), NOW).line).toBe("$2.58 spent");
	});

	it("leads with the account's balance once an admin key reads it", () => {
		const shown = creditsView(
			read({
				limit_cents: 1_000,
				remaining_cents: 742,
				resets: "monthly",
				balance_cents: 1_484,
				bought_cents: 2_000,
			}),
			NOW
		);
		expect(shown.bars.map((b) => [b.window.label, b.text])).toEqual([
			["Account", "$14.84 left of $20.00"],
		]);
		expect(shown.bars[0]!.window.percent).toBeCloseTo(25.8);
		expect(shown.status).toBe("This key: $2.58");
		expect(shown.line).toBe("$14.84 left of $20.00 · This key: $2.58");
		expect(shown.short).toBe("$14.84 left");
		// A spent balance draws as used up.
		const spent = creditsView(read({ balance_cents: 0, bought_cents: 500 }), NOW);
		expect(spent.bars[0]!.window).toMatchObject({ percent: 100, usedUp: true });
	});

	it("keeps the last figures after a failed read, dimmed, with why in the title", () => {
		const shown = creditsView(
			read({ at: NOW - 5 * 60 * MIN, error: "Couldn't reach OpenRouter: it timed out." }),
			NOW
		);
		expect(shown.stale).toBe(true);
		expect(shown.short).toBe("$2.58 spent");
		expect(shown.title).toMatch(
			/^Couldn't reach OpenRouter: it timed out\. These figures are from .+\.$/
		);
	});

	it("names when a limit resets, and passes on a name it doesn't know", () => {
		expect(resetsText(null)).toBe("");
		expect(resetsText(undefined)).toBe("");
		expect(resetsText("daily")).toBe("Resets daily");
		expect(resetsText("weekly")).toBe("Resets weekly");
		expect(resetsText("monthly")).toBe("Resets monthly");
		expect(resetsText("yearly")).toBe("Resets: yearly");
	});
});

describe("where the credits show", () => {
	it("lists the servers with credits by name, and leaves out the others", async () => {
		const rows = serverCreditRows(await load(), NOW);
		expect(rows.map((r) => [r.server.id, r.view.short, r.view.stale])).toEqual([
			[OPENROUTER_SERVER, "$7.42 left", false],
			[DEVBOX_OPENROUTER_SERVER, "$4.12 spent", true],
		]);
		const none = await load((entities) =>
			entities.map((e) => (e.entity === "model_server" ? { ...e, credits: null } : e))
		);
		expect(serverCreditRows(none, NOW)).toEqual([]);
	});

	it("gives the picker the main figure, or nothing without credits", async () => {
		const view = await load();
		expect(creditsShort(view.tables.model_server.get(OPENROUTER_SERVER)!)).toBe("$7.42 left");
		expect(creditsShort(view.tables.model_server.get(MAC_SERVER)!)).toBeNull();
		expect(creditsShort({})).toBeNull();
	});
});

describe("the admin key", () => {
	it("is offered on an OpenRouter server with a token, from the version that keeps one", async () => {
		const view = await load();
		const server = (id: string) => view.tables.model_server.get(id)!;
		expect(offersAdminKey(view, MAC, server(OPENROUTER_SERVER))).toBe(true);
		expect(offersAdminKey(view, DEVBOX, server(DEVBOX_OPENROUTER_SERVER))).toBe(true);
		expect(offersAdminKey(view, DEVBOX, server(OLLAMA_SERVER))).toBe(false);
		expect(offersAdminKey(view, MAC, { ...server(OPENROUTER_SERVER), token: false })).toBe(
			false
		);
		const older = await speaking(MAC, CLOUD_SERVERS_SINCE - 1);
		const kept = older.tables.model_server.get(OPENROUTER_SERVER)!;
		expect(offersAdminKey(older, MAC, kept)).toBe(false);
		// One kept already shows, so it can be removed.
		expect(offersAdminKey(older, MAC, { ...kept, admin_key: true })).toBe(true);
	});

	it("sends the key trimmed, and nothing to remove it", () => {
		expect(setAdminKeyAction(OPENROUTER_SERVER, "  sk-or-v1-example \n")).toEqual({
			action: "set_server_admin_key",
			server: OPENROUTER_SERVER,
			key: "sk-or-v1-example",
		});
		expect(setAdminKeyAction(OPENROUTER_SERVER, "   ")).toMatchObject({ key: null });
		expect(removeAdminKeyAction(OPENROUTER_SERVER)).toEqual({
			action: "set_server_admin_key",
			server: OPENROUTER_SERVER,
			key: null,
		});
	});

	it("says what an admin key can do", () => {
		expect(adminKeyWarning()).toBe(
			"An admin key can create and delete keys on your OpenRouter account. MaxHQ keeps it on this device and uses it only to read your balance."
		);
	});
});
