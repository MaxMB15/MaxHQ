// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { APP_STORE_ID_ENV, RELEASES_REPO_ENV, releaseExtra } from "./release-config.ts";

describe("releaseExtra", () => {
	it("leaves both out of a local build", () => {
		expect(releaseExtra({})).toEqual({});
		expect(releaseExtra({ [RELEASES_REPO_ENV]: "", [APP_STORE_ID_ENV]: "" })).toEqual({});
	});

	it("passes the workflow's repository and App Store ID through", () => {
		expect(
			releaseExtra({ [RELEASES_REPO_ENV]: "owner/repo", [APP_STORE_ID_ENV]: "1234567890" })
		).toEqual({ releasesRepo: "owner/repo", appStoreId: "1234567890" });
		expect(releaseExtra({ [RELEASES_REPO_ENV]: "owner/repo" })).toEqual({
			releasesRepo: "owner/repo",
		});
	});

	it("refuses a malformed value and names the variable", () => {
		expect(() => releaseExtra({ [RELEASES_REPO_ENV]: "https://example.com/x" })).toThrow(
			RELEASES_REPO_ENV
		);
		expect(() => releaseExtra({ [APP_STORE_ID_ENV]: "com.example.maxhq" })).toThrow(
			APP_STORE_ID_ENV
		);
	});
});
