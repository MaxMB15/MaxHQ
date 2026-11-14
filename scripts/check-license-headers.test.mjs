// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const script = join(dirname(fileURLToPath(import.meta.url)), "check-license-headers.sh");

// Built from parts, so this file doesn't trip the check itself.
const OLD_ID = ["LicenseRef", "MaxHQ", "Proprietary"].join("-");
const RESERVED = ["All", "rights", "reserved."].join(" ");

/** An empty repo in a scratch folder, and an environment whose git sees only it. */
const repo = (t) => {
	const root = realpathSync(mkdtempSync(join(tmpdir(), "check-license-headers-")));
	t.after(() => rmSync(root, { recursive: true, force: true }));
	// The commit hook sets GIT_DIR and friends, which would aim these commands
	// at the branch being committed.
	const env = Object.fromEntries(
		Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_"))
	);
	Object.assign(env, { HOME: root, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" });
	execFileSync("git", ["init", "-q"], { cwd: root, env });
	return { root, env };
};

/** Writes `files` and stages them. */
const stage = ({ root, env }, files) => {
	for (const [path, text] of Object.entries(files)) {
		mkdirSync(dirname(join(root, path)), { recursive: true });
		writeFileSync(join(root, path), text);
	}
	execFileSync("git", ["add", "--", ...Object.keys(files)], { cwd: root, env });
};

/** Runs the check from `cwd`, the repo's top unless given. */
const check = ({ root, env }, cwd = root) => {
	return spawnSync("sh", [script], { cwd, env, encoding: "utf8" });
};

test("MIT headers pass", (t) => {
	const r = repo(t);
	stage(r, { "src/a.rs": "// SPDX-License-Identifier: MIT\n// Copyright (c) 2026 Max Boksem\n" });
	assert.equal(check(r).status, 0);
});

test("the old SPDX id fails and names the file", (t) => {
	const r = repo(t);
	stage(r, { "src/a.ts": `// SPDX-License-Identifier: ${OLD_ID}\n` });
	const run = check(r);
	assert.equal(run.status, 1);
	assert.match(run.stderr, /src\/a\.ts:1:/);
});

test("a line reserving all rights fails, in any case", (t) => {
	const r = repo(t);
	stage(r, { "a.css": `/* Copyright (C) 2026 Max Boksem. ${RESERVED.toLowerCase()} */\n` });
	assert.equal(check(r).status, 1);
});

test("THIRD_PARTY_NOTICES may quote a license that reserves rights", (t) => {
	const r = repo(t);
	stage(r, { THIRD_PARTY_NOTICES: `Copyright (c) Someone. ${RESERVED}\n` });
	assert.equal(check(r).status, 0);
});

test("THIRD_PARTY_NOTICES still can't hold the old SPDX id", (t) => {
	const r = repo(t);
	stage(r, { THIRD_PARTY_NOTICES: `${OLD_ID}\n` });
	assert.equal(check(r).status, 1);
});

test("only what's staged counts, from any folder", (t) => {
	const r = repo(t);
	stage(r, { "src/a.rs": "// SPDX-License-Identifier: MIT\n" });
	writeFileSync(join(r.root, "untracked.rs"), `// ${OLD_ID}\n`);
	assert.equal(check(r, join(r.root, "src")).status, 0);
	stage(r, { "nested/deep/b.sh": `# ${RESERVED}\n` });
	const run = check(r, join(r.root, "src"));
	assert.equal(run.status, 1);
	assert.match(run.stderr, /nested\/deep\/b\.sh:1:/);
});
