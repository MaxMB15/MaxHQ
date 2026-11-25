// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { CLOUD_SERVERS_SINCE, type Entity, type Request } from "../bindings";
import { mockSource } from "../data/mock-source";
import { DEVBOX, MAC } from "../data/mock-view";
import type { Ask } from "../data/request-source";
import { NOW } from "../test/build";
import { applySnapshot, type ViewState } from "../view/replica";

import {
	canSignInWithOpenRouter,
	isOpenRouter,
	MAX_LIMIT_CENTS,
	OPENROUTER_URL,
	openRouterCalls,
	openRouterServerOf,
	roundBroke,
	roundEnded,
	spendingLimit,
} from "./openrouter";

/** The sample view with its entities changed. */
const edited = async (edit: (entities: Entity[]) => Entity[] = (e) => e): Promise<ViewState> => {
	const snapshot = await mockSource({ now: () => NOW, tick: false }).snapshot();
	return applySnapshot({ ...snapshot, entities: edit(snapshot.entities) });
};

/** The sample view with a device's `maxd` at `protocol`. */
const at = (device: string, protocol: number) =>
	edited((entities) =>
		entities.map((e) =>
			e.entity === "device_link" && e.device === device ? { ...e, protocol } : e
		)
	);

const external = (base_url: string | null) => ({
	kind: { type: "external", flavor: "other" } as const,
	base_url,
});

describe("OpenRouter's server", () => {
	it("knows OpenRouter's URL however it was typed", () => {
		expect(isOpenRouter(external(OPENROUTER_URL))).toBe(true);
		expect(isOpenRouter(external("https://openrouter.ai/api/v1/"))).toBe(true);
		expect(isOpenRouter(external(" https://OpenRouter.ai/api/ "))).toBe(true);
		expect(isOpenRouter(external("https://openrouter.ai"))).toBe(false);
		expect(isOpenRouter(external("http://devbox:1234/v1"))).toBe(false);
		expect(isOpenRouter(external(null))).toBe(false);
		expect(isOpenRouter({ kind: { type: "managed" }, base_url: OPENROUTER_URL })).toBe(false);
	});

	it("finds the device's OpenRouter server, if it has one", async () => {
		// The sample's Mac runs a server of its own but has no OpenRouter.
		expect(openRouterServerOf(await edited(), MAC)).toBeUndefined();
		const view = await edited((entities) => [
			...entities,
			{
				entity: "model_server",
				id: "srv-mac-openrouter",
				device: MAC,
				...external(OPENROUTER_URL),
				label: "OpenRouter",
				token: true,
				probe: null,
				models: [],
			},
		]);
		expect(openRouterServerOf(view, MAC)?.id).toBe("srv-mac-openrouter");
		expect(openRouterServerOf(view, DEVBOX)).toBeUndefined();
	});

	it("signs in only through a maxd that can", async () => {
		expect(canSignInWithOpenRouter(await at(MAC, CLOUD_SERVERS_SINCE), MAC)).toBe(true);
		expect(canSignInWithOpenRouter(await at(MAC, CLOUD_SERVERS_SINCE - 1), MAC)).toBe(false);
		const unlinked = await edited((entities) =>
			entities.filter((e) => !(e.entity === "device_link" && e.device === DEVBOX))
		);
		expect(canSignInWithOpenRouter(unlinked, DEVBOX)).toBe(false);
	});
});

describe("the spending limit", () => {
	it("reads dollars as cents, and empty as no limit", () => {
		expect(spendingLimit("")).toEqual({ ok: true, cents: null });
		expect(spendingLimit("  ")).toEqual({ ok: true, cents: null });
		expect(spendingLimit("25")).toEqual({ ok: true, cents: 2500 });
		expect(spendingLimit("$12.5")).toEqual({ ok: true, cents: 1250 });
		expect(spendingLimit("$ 12,05")).toEqual({ ok: true, cents: 1205 });
		expect(spendingLimit("0.01")).toEqual({ ok: true, cents: 1 });
		expect(spendingLimit("999999.99")).toEqual({ ok: true, cents: MAX_LIMIT_CENTS });
	});

	it("says what's wrong with one OpenRouter doesn't take", () => {
		expect(spendingLimit("ten")).toEqual({
			ok: false,
			problem: "Type an amount in dollars, such as 25 or 12.50.",
		});
		expect(spendingLimit("1.234")).toMatchObject({ ok: false });
		expect(spendingLimit("-5")).toMatchObject({ ok: false });
		expect(spendingLimit("0")).toEqual({
			ok: false,
			problem: "Type an amount above 0, or leave it empty for no limit.",
		});
		expect(spendingLimit("1000000")).toEqual({
			ok: false,
			problem: "OpenRouter takes a limit below one million dollars.",
		});
	});
});

describe("a sign-in round", () => {
	it("ends signed in, with the server that keeps the key", () => {
		expect(
			roundEnded(
				{ status: "ok", data: { status: "model_server_added", server: "srv-1" } },
				false
			)
		).toEqual({ state: "signed_in", server: "srv-1" });
	});

	it("says why it failed, as a sentence", () => {
		expect(
			roundEnded(
				{ status: "ok", data: { status: "refused", reason: "OpenRouter said no." } },
				false
			)
		).toEqual({ state: "failed", reason: "OpenRouter said no." });
		expect(
			roundEnded(
				{
					status: "error",
					error: { kind: "failed", message: "Port 3000 on this Mac is busy" },
				},
				false
			)
		).toEqual({ state: "failed", reason: "Port 3000 on this Mac is busy." });
		expect(roundBroke()).toEqual({
			state: "failed",
			reason: "Couldn't finish the sign-in. Try again.",
		});
	});

	it("goes back to the start once you cancelled it", () => {
		expect(
			roundEnded(
				{
					status: "error",
					error: { kind: "failed", message: "You cancelled the sign-in" },
				},
				true
			)
		).toEqual({ state: "idle" });
	});
});

describe("the calls", () => {
	it("sends a round and its cancel to the app", async () => {
		const sent: Request[] = [];
		const ask = (async (request: Request) => {
			sent.push(request);
			return { status: "ok", data: null };
		}) as Ask;
		const calls = openRouterCalls(ask);
		await calls.signInOpenRouter(MAC, 2500);
		await calls.signInOpenRouter(DEVBOX, null);
		await calls.cancelSignIn();
		expect(sent).toEqual([
			{ kind: "sign_in_openrouter", device: MAC, label: null, limit_cents: 2500 },
			{ kind: "sign_in_openrouter", device: DEVBOX, label: null, limit_cents: null },
			{ kind: "cancel_sign_in" },
		]);
	});
});
