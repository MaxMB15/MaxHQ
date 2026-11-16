// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { PROTOCOL_VERSION } from "@maxhq/shared/bindings";
import { entities, link } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import {
	afterCheck,
	appTooOld,
	CHECK_EVERY,
	DEFAULT_PREFS,
	dueForCheck,
	isNewer,
	manifestUrl,
	newestNote,
	offeredVersion,
	outcomeOf,
	parseManifest,
	readPrefs,
	testflightLinks,
	tooNewFor,
	type UpdatePrefs,
} from "./updates";

const MANIFEST = {
	version: "0.1.3",
	build: 10702,
	protocol: { current: 45, oldest: 20 },
	notes_url: "https://github.com/owner/repo/releases/tag/v0.1.3",
	pub_date: "2026-11-16T10:11:12Z",
};

const NEWEST = {
	version: "0.1.3",
	build: 10702,
	protocol: { current: 45, oldest: 20 },
	notesUrl: "https://github.com/owner/repo/releases/tag/v0.1.3",
};

const found: UpdatePrefs = { ...DEFAULT_PREFS, checkedAt: 1_000, newest: NEWEST };

describe("mobile.json", () => {
	it("comes from the repository's latest release", () => {
		expect(manifestUrl("owner/repo")).toBe(
			"https://github.com/owner/repo/releases/latest/download/mobile.json"
		);
	});

	it("reads what the release script writes", () => {
		expect(parseManifest(JSON.stringify(MANIFEST))).toEqual(NEWEST);
	});

	it("refuses a manifest that's missing a field or isn't JSON", () => {
		expect(parseManifest("not json")).toBeNull();
		expect(parseManifest("null")).toBeNull();
		expect(parseManifest(JSON.stringify({ ...MANIFEST, version: "0.1" }))).toBeNull();
		expect(parseManifest(JSON.stringify({ ...MANIFEST, build: "7" }))).toBeNull();
		expect(
			parseManifest(JSON.stringify({ ...MANIFEST, protocol: { current: 45 } }))
		).toBeNull();
		expect(
			parseManifest(JSON.stringify({ ...MANIFEST, notes_url: "javascript:alert(1)" }))
		).toBeNull();
	});

	it("turns a check's answer into an outcome", () => {
		expect(outcomeOf(200, JSON.stringify(MANIFEST))).toEqual({
			kind: "found",
			manifest: NEWEST,
		});
		expect(outcomeOf(404, "Not Found")).toEqual({ kind: "none" });
		expect(outcomeOf(503, "")).toEqual({ kind: "failed", reason: "HTTP 503" });
		expect(outcomeOf(200, "{}").kind).toBe("failed");
	});

	it("keeps the last good answer through a failed check", () => {
		expect(afterCheck(found, { kind: "failed", reason: "offline" }, 5_000)).toBe(found);
		expect(afterCheck(found, { kind: "none" }, 5_000)).toEqual({
			...found,
			checkedAt: 5_000,
			newest: null,
		});
		expect(afterCheck(DEFAULT_PREFS, { kind: "found", manifest: NEWEST }, 5_000)).toEqual({
			...DEFAULT_PREFS,
			checkedAt: 5_000,
			newest: NEWEST,
		});
	});
});

describe("isNewer", () => {
	it("compares each part as a number", () => {
		expect(isNewer("0.1.3", "0.1.2")).toBe(true);
		expect(isNewer("0.1.10", "0.1.9")).toBe(true);
		expect(isNewer("1.0.0", "0.99.99")).toBe(true);
		expect(isNewer("0.2.0", "0.10.0")).toBe(false);
	});

	it("isn't newer when equal, older or malformed", () => {
		expect(isNewer("0.1.2", "0.1.2")).toBe(false);
		expect(isNewer("0.1.1", "0.1.2")).toBe(false);
		expect(isNewer("0.1.3-beta", "0.1.2")).toBe(false);
		expect(isNewer("0.1.3", "dev")).toBe(false);
	});
});

describe("the update banner", () => {
	it("offers a newer version", () => {
		expect(offeredVersion(found, "0.1.2")).toBe("0.1.3");
	});

	it("hides once you dismiss that version, and comes back for the next", () => {
		const dismissed = { ...found, dismissed: "0.1.3" };
		expect(offeredVersion(dismissed, "0.1.2")).toBeNull();
		const next = { ...dismissed, newest: { ...NEWEST, version: "0.1.4" } };
		expect(offeredVersion(next, "0.1.2")).toBe("0.1.4");
	});

	it("stays away when this build is current, or nothing is known", () => {
		expect(offeredVersion(found, "0.1.3")).toBeNull();
		expect(offeredVersion(found, "0.2.0")).toBeNull();
		expect(offeredVersion(found, null)).toBeNull();
		expect(offeredVersion(DEFAULT_PREFS, "0.1.2")).toBeNull();
	});
});

describe("kept prefs", () => {
	it("checks when the app opens by default", () => {
		expect(readPrefs(null)).toEqual(DEFAULT_PREFS);
		expect(DEFAULT_PREFS.autoCheck).toBe(true);
	});

	it("round-trips what the store writes", () => {
		const prefs = { ...found, autoCheck: false, dismissed: "0.1.3" };
		expect(readPrefs(JSON.stringify(prefs))).toEqual(prefs);
	});

	it("falls back to the defaults field by field", () => {
		expect(readPrefs("garbage")).toEqual(DEFAULT_PREFS);
		expect(readPrefs(JSON.stringify({ autoCheck: "no", dismissed: 3, newest: {} }))).toEqual(
			DEFAULT_PREFS
		);
	});

	it("checks on open when it's on and the last check is old", () => {
		expect(dueForCheck(DEFAULT_PREFS, 0)).toBe(true);
		expect(dueForCheck(found, found.checkedAt! + CHECK_EVERY - 1)).toBe(false);
		expect(dueForCheck(found, found.checkedAt! + CHECK_EVERY)).toBe(true);
		expect(dueForCheck({ ...DEFAULT_PREFS, autoCheck: false }, 0)).toBe(false);
	});
});

describe("the line under Newest", () => {
	const idle = { ...found, checking: false, failure: null };

	it("says when the phone last looked", () => {
		expect(newestNote(idle, "owner/repo", found.checkedAt! + 2 * 60_000)).toBe(
			"Checked 2 min ago"
		);
	});

	it("says why it knows of no newest build", () => {
		const repo = "owner/repo";
		expect(newestNote(idle, null, 0)).toBe("This build doesn't look for updates.");
		expect(newestNote({ ...idle, checking: true }, repo, 0)).toBe("Checking for updates…");
		expect(newestNote({ ...idle, failure: "HTTP 503" }, repo, 0)).toBe(
			"Couldn't check for updates: HTTP 503"
		);
		expect(newestNote({ ...idle, checkedAt: null }, repo, 0)).toBe(
			"MaxHQ hasn't checked for updates yet."
		);
		expect(newestNote({ ...idle, newest: null }, repo, 0)).toBe(
			"The newest release has no iPhone build."
		);
	});
});

describe("testflightLinks", () => {
	it("opens the app's page first when its ID is known", () => {
		expect(testflightLinks("1234567890")).toEqual([
			"itms-beta://beta.itunes.apple.com/v1/app/1234567890",
			"itms-beta://",
		]);
	});

	it("opens TestFlight itself without one", () => {
		expect(testflightLinks(null)).toEqual(["itms-beta://"]);
		expect(testflightLinks("not-an-id")).toEqual(["itms-beta://"]);
	});
});

describe("the protocol check", () => {
	const ahead = { current: PROTOCOL_VERSION + 1, oldest: PROTOCOL_VERSION + 1 };
	const behind = { current: 19, oldest: 10 };

	it("finds a maxd that speaks only newer protocols", () => {
		expect(tooNewFor(link({ id: "mac", protocol_mismatch: ahead }))).toBe(true);
	});

	it("leaves out an older maxd and a link without a mismatch", () => {
		expect(tooNewFor(link({ id: "old", protocol_mismatch: behind }))).toBe(false);
		expect(tooNewFor(link({ id: "fine" }))).toBe(false);
		expect(tooNewFor(link({ id: "fine", protocol_mismatch: null }))).toBe(false);
	});

	it("names each such device with the protocol its maxd speaks", () => {
		const view = applySnapshot({
			seq: 1,
			entities: entities({
				links: [
					link({ id: "work", name: "Work Mac", protocol_mismatch: ahead }),
					link({ id: "box", name: "devbox", protocol_mismatch: behind }),
					link({ id: "home", name: "Home Mac" }),
				],
			}),
		});
		expect(appTooOld(view)).toEqual([
			{ link: "work", name: "Work Mac", theirs: PROTOCOL_VERSION + 1 },
		]);
	});
});
