// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What app.config.ts puts in a build for the app to find its updates: the
// GitHub repository whose latest release carries mobile.json, and the app's
// App Store ID, which opens its page in TestFlight. The release workflow sets
// both in the environment, so no tracked file names either. A build without
// the repository doesn't look for updates.

export const RELEASES_REPO_ENV = "MAXHQ_RELEASES_REPO";
export const APP_STORE_ID_ENV = "MAXHQ_APP_STORE_ID";

export type ReleaseExtra = { releasesRepo?: string; appStoreId?: string };

const REPO = /^[\w.-]+\/[\w.-]+$/;
const APP_STORE_ID = /^[0-9]{6,12}$/;

// The fields for Expo's `extra`, each left out when unset, since the manifest
// turns null into {}. A malformed value is an error that names the variable.
export const releaseExtra = (env: Record<string, string | undefined>): ReleaseExtra => {
	const repo = env[RELEASES_REPO_ENV] || undefined;
	const appStoreId = env[APP_STORE_ID_ENV] || undefined;
	if (repo !== undefined && !REPO.test(repo)) {
		throw new Error(`${RELEASES_REPO_ENV} is owner/name, such as owner/repo.`);
	}
	if (appStoreId !== undefined && !APP_STORE_ID.test(appStoreId)) {
		throw new Error(
			`${APP_STORE_ID_ENV} is the app's Apple ID from App Store Connect, all digits.`
		);
	}
	return {
		...(repo ? { releasesRepo: repo } : {}),
		...(appStoreId ? { appStoreId } : {}),
	};
};
