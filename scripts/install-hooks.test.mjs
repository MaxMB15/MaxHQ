// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = join(dirname(fileURLToPath(import.meta.url)), "install-hooks.sh");

/** A scratch folder and an environment whose git sees only what's in it. */
const scratch = (t) => {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "install-hooks-")));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	// The commit hook sets GIT_DIR and friends, which would aim these commands
	// at the branch being committed.
	const env = Object.fromEntries(
		Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_"))
	);
	return {
		root,
		env: { ...env, HOME: root, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" },
	};
};

const git = (env, cwd, ...args) => {
	return execFileSync("git", args, { cwd, env, stdio: "pipe", encoding: "utf8" }).trim();
};

/** Copies the script to `checkout` and runs it from there, as pnpm does. */
const install = (env, checkout) => {
	mkdirSync(join(checkout, "scripts"), { recursive: true });
	cpSync(script, join(checkout, "scripts/install-hooks.sh"));
	execFileSync("sh", ["scripts/install-hooks.sh"], { cwd: checkout, env, stdio: "pipe" });
};

/** The repo's `core.hooksPath`, or an empty string when it has none. */
const hooksPath = (env, repo) => {
	const asked = ["config", "--local", "--get", "core.hooksPath"];
	return spawnSync("git", asked, { cwd: repo, env, encoding: "utf8" }).stdout.trim();
};

test("a checkout of its own gets the hooks", (t) => {
	const { root, env } = scratch(t);
	git(env, root, "init");
	install(env, root);
	assert.equal(hooksPath(env, root), ".githooks");
});

test("a copy inside another repo leaves that repo's hooks alone", (t) => {
	const { root, env } = scratch(t);
	git(env, root, "init");
	install(env, join(root, "vendor/maxhq"));
	assert.equal(hooksPath(env, root), "");
});

test("a linked worktree gets the hooks", (t) => {
	const { root, env } = scratch(t);
	const main = join(root, "main");
	const linked = join(root, "linked");
	mkdirSync(main);
	git(env, main, "init");
	const name = ["-c", "user.name=Test", "-c", "user.email=test@example.com"];
	git(env, main, ...name, "commit", "--allow-empty", "-m", "first");
	git(env, main, "worktree", "add", linked);
	install(env, linked);
	assert.equal(hooksPath(env, linked), ".githooks");
});

test("a folder outside any repo has nothing to set", (t) => {
	const { root, env } = scratch(t);
	install(env, root);
	assert.equal(hooksPath(env, root), "");
});
