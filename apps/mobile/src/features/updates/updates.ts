// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone knows about its own updates. Each release names its iPhone
// build in mobile.json, next to the Mac's latest.json; the phone reads it
// from the newest release, offers a newer version in TestFlight, and
// remembers the version whose banner you dismissed. It also finds the
// devices whose maxd shares no protocol with this app, and which side is
// older.

import { t } from "@maxhq/shared/i18n";

import {
	type DeviceLink,
	OLDEST_PROTOCOL_VERSION,
	PROTOCOL_VERSION,
	type ProtocolRange,
} from "@maxhq/shared/bindings";
import { formatAgo } from "@maxhq/shared/status/format";
import { all, type ViewState } from "@maxhq/shared/view/replica";

/** The newest release's iPhone build, as its mobile.json names it. */
export type Manifest = {
	readonly version: string;
	readonly build: number;
	readonly protocol: { readonly current: number; readonly oldest: number };
	readonly notesUrl: string;
};

/**
 * Where a build finds mobile.json: the latest release of the repository
 * the release workflow built it from, as the Mac finds latest.json.
 */
export const manifestUrl = (repo: string): string =>
	`https://github.com/${repo}/releases/latest/download/mobile.json`;

const SEMVER = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;

const count = (value: unknown): value is number =>
	typeof value === "number" && Number.isSafeInteger(value) && value >= 0;

/** A manifest's fields, checked, or null when one is missing or wrong. */
const manifestOf = (value: unknown): Manifest | null => {
	if (!value || typeof value !== "object") return null;
	const { version, build, protocol, notesUrl } = value as Record<string, unknown>;
	if (typeof version !== "string" || !SEMVER.test(version) || !count(build)) return null;
	const range = (protocol ?? {}) as Record<string, unknown>;
	if (!count(range.current) || !count(range.oldest)) return null;
	if (typeof notesUrl !== "string" || !notesUrl.startsWith("https://")) return null;
	return { version, build, protocol: { current: range.current, oldest: range.oldest }, notesUrl };
};

/** mobile.json's text as a manifest, or null when it isn't one. */
export const parseManifest = (text: string): Manifest | null => {
	try {
		const value = JSON.parse(text) as Record<string, unknown> | null;
		return manifestOf(value && { ...value, notesUrl: value.notes_url });
	} catch {
		return null;
	}
};

/**
 * Whether version `a` comes after `b`, both X.Y.Z. Anything else is never
 * newer, so a malformed version never offers an update.
 */
export const isNewer = (a: string, b: string): boolean => {
	const left = SEMVER.exec(a);
	const right = SEMVER.exec(b);
	if (!left || !right) return false;
	for (let part = 1; part <= 3; part++) {
		const difference = Number(left[part]) - Number(right[part]);
		if (difference !== 0) return difference > 0;
	}
	return false;
};

/** What the phone keeps about its updates between launches. */
export type UpdatePrefs = {
	/** Whether it also checks when the app opens. */
	readonly autoCheck: boolean;
	/** The version whose banner was dismissed. */
	readonly dismissed: string | null;
	/** When the last check answered, in ms. */
	readonly checkedAt: number | null;
	/** What the last check found, or null when the newest release had no iPhone build. */
	readonly newest: Manifest | null;
};

export const DEFAULT_PREFS: UpdatePrefs = {
	autoCheck: true,
	dismissed: null,
	checkedAt: null,
	newest: null,
};

/** The kept file's text as prefs, the defaults filling what's missing or wrong. */
export const readPrefs = (text: string | null): UpdatePrefs => {
	if (!text) return DEFAULT_PREFS;
	let value: Record<string, unknown>;
	try {
		value = (JSON.parse(text) ?? {}) as Record<string, unknown>;
	} catch {
		return DEFAULT_PREFS;
	}
	return {
		autoCheck: typeof value.autoCheck === "boolean" ? value.autoCheck : true,
		dismissed: typeof value.dismissed === "string" ? value.dismissed : null,
		checkedAt: count(value.checkedAt) ? value.checkedAt : null,
		newest: manifestOf(value.newest),
	};
};

/** What a check found. */
export type CheckOutcome =
	| { readonly kind: "found"; readonly manifest: Manifest }
	/** The newest release has no iPhone build. */
	| { readonly kind: "none" }
	| { readonly kind: "failed"; readonly reason: string };

/**
 * A check's answer from mobile.json's HTTP status and text. A release made
 * before the iPhone build joined the release has no mobile.json, so GitHub
 * answers 404.
 */
export const outcomeOf = (status: number, text: string): CheckOutcome => {
	if (status === 404) return { kind: "none" };
	if (status < 200 || status >= 300) return { kind: "failed", reason: `HTTP ${status}` };
	const manifest = parseManifest(text);
	return manifest
		? { kind: "found", manifest }
		: { kind: "failed", reason: t("the release's mobile.json isn't valid") };
};

/** The prefs after a check. A failed one keeps what the last good one found. */
export const afterCheck = (prefs: UpdatePrefs, outcome: CheckOutcome, now: number): UpdatePrefs =>
	outcome.kind === "failed"
		? prefs
		: { ...prefs, checkedAt: now, newest: outcome.kind === "found" ? outcome.manifest : null };

/** The prefs, and whether a check is running or the last one failed, which aren't kept. */
export type CheckState = UpdatePrefs & {
	readonly checking: boolean;
	readonly failure: string | null;
};

/**
 * The line under Newest in About: when the phone last looked, or why it
 * knows of no newest build. `repo` is null in a build that doesn't look.
 */
export const newestNote = (state: CheckState, repo: string | null, now: number): string => {
	if (!repo) return t("This build doesn't look for updates.");
	if (state.checking) return t("Checking for updates…");
	if (state.failure)
		return t("Couldn't check for updates: {{reason}}", { reason: state.failure });
	if (state.checkedAt === null) return t("MaxHQ hasn't checked for updates yet.");
	if (!state.newest) return t("The newest release has no iPhone build.");
	return t("Checked {{ago}}", { ago: formatAgo(state.checkedAt, now) });
};

/** How long a check counts as fresh when the app comes back to the front. */
export const CHECK_EVERY = 60 * 60 * 1000;

/** Whether opening the app should check now. */
export const dueForCheck = (prefs: UpdatePrefs, now: number): boolean =>
	prefs.autoCheck && (prefs.checkedAt === null || now - prefs.checkedAt >= CHECK_EVERY);

/**
 * The version the update banner offers: the newest release's, when it's
 * newer than this build and its banner wasn't dismissed. Null otherwise.
 */
export const offeredVersion = (prefs: UpdatePrefs, current: string | null): string | null => {
	const newest = prefs.newest?.version;
	if (!newest || !current || !isNewer(newest, current)) return null;
	return newest === prefs.dismissed ? null : newest;
};

/**
 * The links that open TestFlight, tried in order: the app's own page when
 * its App Store ID is known, then TestFlight itself.
 */
export const testflightLinks = (appStoreId: string | null): string[] => [
	...(appStoreId && /^[0-9]+$/.test(appStoreId)
		? [`itms-beta://beta.itunes.apple.com/v1/app/${appStoreId}`]
		: []),
	"itms-beta://",
];

/** The protocols this app speaks. */
const OURS: ProtocolRange = { current: PROTOCOL_VERSION, oldest: OLDEST_PROTOCOL_VERSION };

/** Which side of a protocol mismatch is older, so which one to update. */
export type OlderSide = "app" | "maxd";

/**
 * The older side when a link's last hello found no protocol in common, or
 * null when it found one or the device didn't say which it speaks.
 */
export const olderSide = (link: DeviceLink, ours: ProtocolRange = OURS): OlderSide | null => {
	const theirs = link.protocol_mismatch;
	if (!theirs) return null;
	if (theirs.oldest > ours.current) return "app";
	if (theirs.current < ours.oldest) return "maxd";
	return null;
};

/** A device whose maxd and this app share no protocol. */
export type Mismatch = {
	readonly link: string;
	readonly name: string;
	/** The newest protocol its maxd speaks. */
	readonly theirs: number;
	readonly older: OlderSide;
};

/** The devices this app can't talk to for their protocol, by name. */
export const mismatches = (view: ViewState, ours: ProtocolRange = OURS): Mismatch[] =>
	all(view, "device_link")
		.flatMap((link): Mismatch[] => {
			const older = olderSide(link, ours);
			return older
				? [
						{
							link: link.id,
							name: link.name,
							theirs: link.protocol_mismatch!.current,
							older,
						},
					]
				: [];
		})
		.sort((a, b) => a.name.localeCompare(b.name));

/** An error's words, for "Couldn't check for updates: …". */
export const reasonOf = (error: unknown): string =>
	error instanceof Error ? error.message : String(error);
