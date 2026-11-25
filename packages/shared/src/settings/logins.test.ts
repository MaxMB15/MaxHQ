// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { type Entity, FCM_SINCE } from "../bindings";
import { account, device, entities, link, MIN, NOW } from "../test/build";
import { credential } from "../test/pulls";
import { applySnapshot } from "../view/replica";

import {
	appTargets,
	deviceFirst,
	fcmTargets,
	fileName,
	folderAccounts,
	heldOf,
	macAccounts,
	macDevice,
	onlyTarget,
	parseAppId,
	pushTargets,
	reachText,
	setupTokenCommand,
	signedInAs,
	type Target,
	withoutWhitespace,
} from "./logins";

const view = (more: Entity[] = []) =>
	applySnapshot({
		seq: 1,
		entities: [
			...entities({
				links: [
					link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
					link({
						id: "devbox",
						device: "dev-devbox",
						capabilities: {
							transfers: true,
							tunnels: true,
							workspaces: true,
							phones: true,
						},
					}),
					link({ id: "alpha", device: "dev-alpha" }),
				],
				devices: [
					device({ id: "dev-mac", name: "Mac", os: "macos" }),
					device({ id: "dev-devbox", name: "devbox" }),
					device({ id: "dev-alpha", name: "alpha" }),
				],
				accounts: [
					account({ id: "work", label: "Work", config_dir: "/Users/me/.claude-work" }),
					account({ id: "home", label: "Home" }),
					account({ id: "box", label: "Box", device: "dev-devbox" }),
					account({ id: "token", label: "Token", login: "setup_token" }),
					account({ id: "cursor", vendor: "cursor", label: "Cursor" }),
				],
			}),
			...more,
		],
	});

describe("setupTokenCommand", () => {
	it("leaves the default folder out", () => {
		expect(setupTokenCommand("/Users/me/.claude")).toBe("claude setup-token");
		expect(setupTokenCommand("/home/me/.claude/")).toBe("claude setup-token");
	});

	it("names any other folder, quoted when it needs quotes", () => {
		expect(setupTokenCommand("/Users/me/.claude-work")).toBe(
			"CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude setup-token"
		);
		expect(setupTokenCommand("/Users/me/My Claude's")).toBe(
			"CLAUDE_CONFIG_DIR='/Users/me/My Claude'\\''s' claude setup-token"
		);
	});
});

describe("typed values", () => {
	it("takes the spaces and line breaks out of a pasted secret", () => {
		expect(withoutWhitespace(" abc\n def\t")).toBe("abcdef");
	});

	it("reads an App ID only when it's a whole number above 0", () => {
		expect(parseAppId(" 1234 567 ")).toBe(1234567);
		expect(parseAppId("0")).toBeNull();
		expect(parseAppId("12a")).toBeNull();
		expect(parseAppId("")).toBeNull();
		expect(parseAppId("1".repeat(13))).toBeNull();
	});

	it("names the file at the end of a path", () => {
		expect(fileName("/Users/me/Downloads/key.pem")).toBe("key.pem");
		expect(fileName("C:\\keys\\key.p8")).toBe("key.p8");
		expect(fileName("key.pem")).toBe("key.pem");
	});
});

describe("accounts", () => {
	it("finds the Mac by its local link", () => {
		expect(macDevice(view())).toBe("dev-mac");
	});

	it("lists the Mac's own folder logins by name", () => {
		expect(macAccounts(view(), "claude").map((a) => a.id)).toEqual(["home", "work"]);
		expect(macAccounts(view(), "cursor").map((a) => a.id)).toEqual(["cursor"]);
	});

	it("lists every device's folder logins by device and then name", () => {
		expect(folderAccounts(view(), "claude").map((a) => a.id)).toEqual(["box", "home", "work"]);
	});

	it("says who a login is signed in as", () => {
		expect(signedInAs(account({ id: "a" }))).toBe("Signed in as someone@example.com");
		expect(signedInAs(account({ id: "a", email: null }))).toBe("Signed in");
	});
});

describe("targets", () => {
	const targets: Target[] = [
		{ device: "dev-alpha", name: "alpha", live: true },
		{ device: "dev-devbox", name: "devbox", live: true },
	];

	it("keeps only the device a page was opened for", () => {
		expect(onlyTarget(targets, "dev-devbox").map((t) => t.device)).toEqual(["dev-devbox"]);
	});

	it("keeps them all when that device isn't one of them", () => {
		expect(onlyTarget(targets, "dev-mac")).toEqual(targets);
		expect(onlyTarget(targets)).toEqual(targets);
	});

	it("lists every device for the App, by name", () => {
		expect(appTargets(view(), NOW).map((t) => t.name)).toEqual(["alpha", "devbox", "Mac"]);
	});

	it("lists only the devices that push to phones for the APNs key", () => {
		expect(pushTargets(view(), NOW).map((t) => t.device)).toEqual(["dev-devbox"]);
	});

	it("blocks a device whose maxd doesn't know FCM from the Firebase key", () => {
		const phones = { transfers: true, tunnels: true, workspaces: true, phones: true };
		const at = (protocol: number | null) =>
			applySnapshot({
				seq: 1,
				entities: entities({
					links: [
						link({
							id: "devbox",
							device: "dev-devbox",
							capabilities: phones,
							protocol,
						}),
					],
					devices: [device({ id: "dev-devbox", name: "devbox" })],
				}),
			});
		expect(fcmTargets(at(FCM_SINCE), NOW)).toEqual([
			{ device: "dev-devbox", name: "devbox", live: true },
		]);
		expect(fcmTargets(at(null), NOW)[0]?.blocked).toBeUndefined();
		expect(fcmTargets(at(FCM_SINCE - 1), NOW)).toEqual([
			{
				device: "dev-devbox",
				name: "devbox",
				live: true,
				blocked: "devbox's maxd is too old for this. Update it first.",
			},
		]);
	});
});

describe("held credentials", () => {
	const v = view([
		credential("dev-devbox", { type: "cursor_key", account: "k1", label: "Box key" }),
		{
			...(credential("dev-alpha", {
				type: "cursor_key",
				account: "k2",
				label: "Alpha key",
			}) as Entity & {
				entity: "credential";
			}),
			stored_at: NOW - 5 * MIN,
		},
		credential("dev-alpha", { type: "chat_gpt", account: "c1", email: null }),
	]);

	it("lists one kind by device", () => {
		expect(heldOf(v, "cursor_key").map((c) => c.kind.label)).toEqual(["Alpha key", "Box key"]);
		expect(heldOf(v, "chat_gpt")).toHaveLength(1);
		expect(heldOf(v, "github_app")).toEqual([]);
	});

	it("puts the device a page was opened for first", () => {
		const held = heldOf(v, "cursor_key");
		expect(deviceFirst(held, "dev-devbox").map((c) => c.device)).toEqual([
			"dev-devbox",
			"dev-alpha",
		]);
		expect(deviceFirst(held).map((c) => c.device)).toEqual(["dev-alpha", "dev-devbox"]);
	});
});

describe("reachText", () => {
	it("lists the repos the App reaches", () => {
		expect(reachText(["me/one", "me/two"])).toBe("Reaches me/one and me/two.");
	});

	it("says to install the App when it reaches none", () => {
		expect(reachText([])).toBe(
			"Reaches no repo yet. Install the App on the repos your projects use."
		);
	});
});
