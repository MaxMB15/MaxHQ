// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { gitLinkOf, sameRepo } from "./git-link";

describe("gitLinkOf", () => {
	it("reads a link to a pull request or an issue", () => {
		expect(gitLinkOf("https://github.com/owner/repo/pull/412")).toEqual({
			repo: "owner/repo",
			number: 412,
		});
		expect(gitLinkOf("https://github.com/owner/repo/issues/7")).toEqual({
			repo: "owner/repo",
			number: 7,
		});
	});

	it("takes the trailing parts a link can have", () => {
		for (const tail of [
			"/",
			"#issuecomment-1",
			"?notification_referrer_id=abc",
			"/files",
			"/commits/",
			"/checks?check_run_id=9#step:1:2",
			"/?a=b",
		])
			expect(gitLinkOf(`https://github.com/owner/repo/pull/412${tail}`), tail).toEqual({
				repo: "owner/repo",
				number: 412,
			});
		expect(gitLinkOf("https://github.com/owner/repo/issues/7/#top")?.number).toBe(7);
	});

	it("trims space around the paste", () => {
		expect(gitLinkOf("  https://github.com/owner/repo/pull/412\n")?.number).toBe(412);
	});

	it("takes names with dots, dashes and underscores", () => {
		expect(gitLinkOf("https://github.com/some-one/my_repo.js/pull/1")?.repo).toBe(
			"some-one/my_repo.js"
		);
		expect(gitLinkOf("https://github.com/a.b/.github/issues/2")?.repo).toBe("a.b/.github");
		expect(gitLinkOf("https://github.com/Owner/Repo/pull/3")?.repo).toBe("Owner/Repo");
	});

	it("takes numbers up to nine digits", () => {
		expect(gitLinkOf("https://github.com/owner/repo/pull/999999999")?.number).toBe(999_999_999);
		for (const number of ["1000000000", "99999999999999999999", "0", "012"])
			expect(gitLinkOf(`https://github.com/owner/repo/pull/${number}`), number).toBeNull();
	});

	it("leaves anything else as text", () => {
		for (const text of [
			"",
			"github.com/owner/repo/pull/1",
			"http://github.com/owner/repo/pull/1",
			"https://www.github.com/owner/repo/pull/1",
			"https://GitHub.com/owner/repo/pull/1",
			"https://gitlab.com/owner/repo/pull/1",
			"https://github.com.example/owner/repo/pull/1",
			"https://github.com/owner/repo/pulls/1",
			"https://github.com/owner/repo/pull/",
			"https://github.com/owner/repo/pull/1a",
			"https://github.com/owner/repo/pull/1/files/extra",
			"https://github.com/owner/repo/pull/1/reviews",
			"https://github.com/owner/repo/issues/1/files",
			"https://github.com/owner/repo/discussions/1",
			"https://github.com/owner/pull/1",
			"https://github.com/owner/repo/tree/main/pull/1",
			"https://github.com/../repo/pull/1",
			"https://github.com/owner/../pull/1",
			"https://github.com/owner/repo/pull/1 and more",
			"see https://github.com/owner/repo/pull/1",
			"https://github.com/owner/repo/pull/1\nhttps://github.com/owner/repo/pull/2",
			"https://github.com/owner/re po/pull/1",
		])
			expect(gitLinkOf(text), text).toBeNull();
	});
});

describe("sameRepo", () => {
	it("reads repo names whatever their case", () => {
		expect(sameRepo("Owner/Repo", "owner/repo")).toBe(true);
		expect(sameRepo("owner/repo", "owner/repo2")).toBe(false);
	});
});
