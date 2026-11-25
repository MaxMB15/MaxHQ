// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { OLDEST_PROTOCOL_VERSION, PROTOCOL_VERSION } from "@maxhq/shared/bindings";
import { entities, link } from "@maxhq/shared/test/build";
import { applySnapshot } from "@maxhq/shared/view/replica";

import {
	afterCheck,
	CHECK_EVERY,
	DEFAULT_PREFS,
	dueForCheck,
	hasNewer,
	isNewer,
	manifestUrl,
	mismatches,
	newestFor,
	newestNote,
	offeredVersion,
	olderSide,
	outcomeOf,
	parseManifest,
	readPrefs,
	storeNote,
	testflightLinks,
	type UpdatePrefs,
	updateLinks,
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
	android: null,
};

const APK = "https://github.com/owner/repo/releases/download/v0.1.3/MaxHQ-0.1.3.apk";

/** What mobile.json says once a release has an Android build. */
const WITH_ANDROID = { ...MANIFEST, android: { build: 10702, apk_url: APK } };

const NEWEST_ANDROID = { ...NEWEST, android: { build: 10702, apkUrl: APK } };

/** A build on the phone, an older release than NEWEST. */
const OLDER = { version: "0.1.2", build: 10600 };

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
		expect(offeredVersion(found, OLDER, "ios")).toBe("0.1.3");
	});

	it("hides once you dismiss that version, and comes back for the next", () => {
		const dismissed = { ...found, dismissed: "0.1.3" };
		expect(offeredVersion(dismissed, OLDER, "ios")).toBeNull();
		const next = { ...dismissed, newest: { ...NEWEST, version: "0.1.4" } };
		expect(offeredVersion(next, OLDER, "ios")).toBe("0.1.4");
	});

	it("stays away when this build is current, or nothing is known", () => {
		expect(offeredVersion(found, { version: "0.1.3", build: 10702 }, "ios")).toBeNull();
		expect(offeredVersion(found, { version: "0.2.0", build: 10900 }, "ios")).toBeNull();
		expect(offeredVersion(found, { version: null, build: null }, "ios")).toBeNull();
		expect(offeredVersion(DEFAULT_PREFS, OLDER, "ios")).toBeNull();
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
		expect(newestNote(idle, "owner/repo", found.checkedAt! + 2 * 60_000, "ios")).toBe(
			"Checked 2 min ago"
		);
	});

	it("says why it knows of no newest build", () => {
		const repo = "owner/repo";
		expect(newestNote(idle, null, 0, "ios")).toBe("This build doesn't look for updates.");
		expect(newestNote({ ...idle, checking: true }, repo, 0, "ios")).toBe(
			"Checking for updates…"
		);
		expect(newestNote({ ...idle, failure: "HTTP 503" }, repo, 0, "ios")).toBe(
			"Couldn't check for updates: HTTP 503"
		);
		expect(newestNote({ ...idle, checkedAt: null }, repo, 0, "ios")).toBe(
			"MaxHQ hasn't checked for updates yet."
		);
		expect(newestNote({ ...idle, newest: null }, repo, 0, "ios")).toBe(
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

describe("an Android build", () => {
	const withApk: UpdatePrefs = { ...found, newest: NEWEST_ANDROID };

	it("reads the release's Android entry", () => {
		expect(parseManifest(JSON.stringify(WITH_ANDROID))).toEqual(NEWEST_ANDROID);
	});

	it("keeps the iPhone build when the Android entry is missing or wrong", () => {
		const wrong = [
			{ build: "10702", apk_url: APK },
			{ build: 10702 },
			{ build: 10702, apk_url: "http://example.com/MaxHQ.apk" },
			{ build: -1, apk_url: APK },
			"10702",
		];
		for (const android of wrong)
			expect(parseManifest(JSON.stringify({ ...MANIFEST, android }))).toEqual(NEWEST);
	});

	it("round-trips through the kept prefs", () => {
		expect(readPrefs(JSON.stringify(withApk))).toEqual(withApk);
	});

	it("is newer by build number, not by version", () => {
		expect(hasNewer(NEWEST_ANDROID, { version: "0.1.3", build: 10600 }, "android")).toBe(true);
		expect(hasNewer(NEWEST_ANDROID, { version: "0.1.2", build: 10702 }, "android")).toBe(false);
		expect(hasNewer(NEWEST_ANDROID, { version: "0.1.2", build: null }, "android")).toBe(false);
	});

	it("is never newer before a release has an Android build", () => {
		expect(hasNewer(NEWEST, OLDER, "android")).toBe(false);
		expect(hasNewer(null, OLDER, "android")).toBe(false);
		// iOS goes on comparing versions alone.
		expect(hasNewer(NEWEST, OLDER, "ios")).toBe(true);
		expect(hasNewer(NEWEST_ANDROID, { version: "0.1.3", build: 10600 }, "ios")).toBe(false);
	});

	it("offers the version, and hides once it's dismissed", () => {
		expect(offeredVersion(withApk, OLDER, "android")).toBe("0.1.3");
		expect(offeredVersion({ ...withApk, dismissed: "0.1.3" }, OLDER, "android")).toBeNull();
		expect(offeredVersion(found, OLDER, "android")).toBeNull();
	});

	it("counts only a release with an Android build as the newest", () => {
		expect(newestFor(NEWEST, "android")).toBeNull();
		expect(newestFor(NEWEST_ANDROID, "android")).toBe(NEWEST_ANDROID);
		expect(newestFor(NEWEST, "ios")).toBe(NEWEST);
	});

	it("says in Android's words that the release has no build for it", () => {
		const idle = { ...found, checking: false, failure: null };
		expect(newestNote(idle, "owner/repo", 0, "android")).toBe(
			"The newest release has no Android build."
		);
		expect(
			newestNote(
				{ ...idle, newest: NEWEST_ANDROID },
				"owner/repo",
				found.checkedAt! + 2 * 60_000,
				"android"
			)
		).toBe("Checked 2 min ago");
	});

	it("downloads the APK, else opens the latest release", () => {
		const build = { appStoreId: "1234567890", repo: "owner/repo" };
		expect(updateLinks("android", build, NEWEST_ANDROID)).toEqual([APK]);
		expect(updateLinks("android", build, NEWEST)).toEqual([
			"https://github.com/owner/repo/releases/latest",
		]);
		expect(updateLinks("android", { appStoreId: null, repo: null }, null)).toEqual([]);
		expect(updateLinks("ios", build, NEWEST_ANDROID)).toEqual(testflightLinks("1234567890"));
	});

	it("says a copy from Google Play updates through Play, on Android only", () => {
		expect(storeNote("android")).toBe(
			"Installed from Google Play? Play updates MaxHQ for you."
		);
		expect(storeNote("ios")).toBeNull();
	});
});

describe("the protocol check", () => {
	const ahead = { current: PROTOCOL_VERSION + 1, oldest: PROTOCOL_VERSION + 1 };
	const behind = { current: OLDEST_PROTOCOL_VERSION - 1, oldest: 10 };

	it("calls the app older when the maxd speaks only newer protocols", () => {
		expect(olderSide(link({ id: "mac", protocol_mismatch: ahead }))).toBe("app");
	});

	it("calls the maxd older when it speaks only protocols this app dropped", () => {
		expect(olderSide(link({ id: "old", protocol_mismatch: behind }))).toBe("maxd");
	});

	it("finds no older side without a mismatch or with a range in common", () => {
		expect(olderSide(link({ id: "fine" }))).toBeNull();
		expect(olderSide(link({ id: "fine", protocol_mismatch: null }))).toBeNull();
		const shared = { current: PROTOCOL_VERSION + 2, oldest: PROTOCOL_VERSION };
		expect(olderSide(link({ id: "stale", protocol_mismatch: shared }))).toBeNull();
	});

	it("compares with the range it's given", () => {
		const ours = { current: 30, oldest: 25 };
		const theirs = (current: number, oldest: number) =>
			link({ id: "mac", protocol_mismatch: { current, oldest } });
		expect(olderSide(theirs(32, 31), ours)).toBe("app");
		expect(olderSide(theirs(24, 20), ours)).toBe("maxd");
		expect(olderSide(theirs(30, 30), ours)).toBeNull();
		expect(olderSide(theirs(25, 20), ours)).toBeNull();
	});

	it("names each such device with its maxd's protocol and the older side", () => {
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
		expect(mismatches(view)).toEqual([
			{ link: "box", name: "devbox", theirs: OLDEST_PROTOCOL_VERSION - 1, older: "maxd" },
			{ link: "work", name: "Work Mac", theirs: PROTOCOL_VERSION + 1, older: "app" },
		]);
	});
});
