// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The phone's updates: this build, read from its Expo config, and what it
// knows of the newest release's iPhone and Android builds. The prefs stay
// in a file in the app's documents, as the dispatcher's do; whether a check
// is running or failed lasts for this run only.

import { t } from "@maxhq/shared/i18n";

import Constants from "expo-constants";
import { File, Paths } from "expo-file-system";
import { Alert, Linking } from "react-native";
import { createStore } from "zustand";

import { platform } from "../../platform";
import {
	afterCheck,
	type CheckOutcome,
	type CheckState,
	DEFAULT_PREFS,
	dueForCheck,
	manifestUrl,
	outcomeOf,
	readPrefs,
	reasonOf,
	type ThisBuild,
	type UpdatePrefs,
	updateLinks,
} from "./updates";

const config = Constants.expoConfig;

const extra = (key: string): string | null => {
	const value: unknown = config?.extra?.[key];
	return typeof value === "string" && value ? value : null;
};

/**
 * The build number: the iPhone's for TestFlight, or the APK's version code,
 * which is the same number on the same release. An APK whose config names no
 * version code falls back to the iPhone's.
 */
const buildNumber = (): number | null => {
	const value =
		platform.updatesThrough === "testflight"
			? config?.ios?.buildNumber
			: (config?.android?.versionCode ?? config?.ios?.buildNumber);
	const number = Number(value);
	return value !== undefined && Number.isSafeInteger(number) && number > 0 ? number : null;
};

/**
 * This build: its version and build number, the repository whose latest
 * release names the newest build, and its App Store ID. A build made on the
 * Mac has neither of the last two, so it doesn't look for updates.
 */
export const thisBuild: ThisBuild & {
	readonly repo: string | null;
	readonly appStoreId: string | null;
} = {
	version: config?.version ?? null,
	build: buildNumber(),
	repo: extra("releasesRepo"),
	appStoreId: extra("appStoreId"),
};

const kept = () => new File(Paths.document, "updates.json");

const load = (): UpdatePrefs => {
	try {
		const file = kept();
		return readPrefs(file.exists ? file.textSync() : null);
	} catch {
		return DEFAULT_PREFS;
	}
};

export const updates = createStore<CheckState>(() => ({
	...load(),
	checking: false,
	failure: null,
}));

const prefsOf = ({ autoCheck, dismissed, checkedAt, newest }: CheckState): UpdatePrefs => ({
	autoCheck,
	dismissed,
	checkedAt,
	newest,
});

const keep = (change: Partial<CheckState>) => {
	updates.setState(change);
	try {
		kept().write(JSON.stringify(prefsOf(updates.getState())));
	} catch {
		// Kept for this run only; the next launch starts from the defaults.
	}
};

/** How long a check waits for GitHub. */
const TIMEOUT = 15_000;

/** Reads mobile.json now, unless a check is running or this build doesn't look. */
export const checkNow = async (): Promise<void> => {
	const repo = thisBuild.repo;
	if (!repo || updates.getState().checking) return;
	updates.setState({ checking: true, failure: null });
	const abort = new AbortController();
	const timer = setTimeout(() => abort.abort(), TIMEOUT);
	let outcome: CheckOutcome;
	try {
		const response = await fetch(manifestUrl(repo), {
			cache: "no-store",
			signal: abort.signal,
		});
		outcome = outcomeOf(response.status, await response.text());
	} catch (error) {
		outcome = {
			kind: "failed",
			reason: abort.signal.aborted ? t("Timed out") : reasonOf(error),
		};
	} finally {
		clearTimeout(timer);
	}
	keep({
		...afterCheck(prefsOf(updates.getState()), outcome, Date.now()),
		checking: false,
		failure: outcome.kind === "failed" ? outcome.reason : null,
	});
};

/** Checks when the app opens or comes back, if that's on and the last check is old. */
export const checkIfDue = () => {
	if (thisBuild.repo && dueForCheck(updates.getState(), Date.now())) void checkNow();
};

/** Keeps whether the phone also checks when the app opens. */
export const keepAutoCheck = (autoCheck: boolean) => keep({ autoCheck });

/** Hides the update banner for this version only. */
export const dismiss = (version: string) => keep({ dismissed: version });

/**
 * Opens where this phone updates: this app's page in TestFlight, else
 * TestFlight, on iOS, and the newest APK's download on Android. Says so when
 * none opens.
 */
export const openUpdate = async (): Promise<void> => {
	for (const url of updateLinks(platform.updatesThrough, thisBuild, updates.getState().newest)) {
		try {
			await Linking.openURL(url);
			return;
		} catch {
			// The next link.
		}
	}
	Alert.alert(t("Couldn't open TestFlight."));
};
