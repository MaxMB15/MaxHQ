// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Fails when an npm package the app ships has a license outside the allowlist
// below. Only production dependencies count, since dev tools such as ESLint
// never reach the bundle. cargo-deny checks the Rust side against deny.toml,
// which holds the same list.

import { execFileSync } from "node:child_process";

const allowed = new Set([
	"0BSD",
	"Apache-2.0",
	"Apache-2.0 WITH LLVM-exception",
	"BSD-2-Clause",
	"BSD-3-Clause",
	"BSL-1.0",
	"BlueOak-1.0.0",
	"CC-BY-4.0",
	"ISC",
	"MIT",
	"MIT-0",
	"MPL-2.0",
	"Python-2.0",
	"Unicode-3.0",
	"Unicode-DFS-2016",
	"Unlicense",
	"Zlib",
]);

// One more license for one package each, and why.
const exceptions = new Map([
	// elkjs is EPL-2.0 OR GPL-3.0-or-later, and P-6 takes the EPL for the
	// workflow canvas layout (F-3) and nothing else.
	["elkjs", "EPL-2.0"],
	// mdn-data is CSS reference data under CC0, which asks nothing of us. It
	// comes with react-native-svg, for the phone's icons.
	["mdn-data", "CC0-1.0"],
]);

// Packages whose package.json names no license, with the license their
// license file grants, read by hand for each version. pnpm looks for a file
// named LICENSE, which finds a lowercase `license` on macOS but not on
// Linux, so CI reports these as Unknown.
const unlabeled = new Map([
	// Mermaid's color library.
	["khroma@2.1.0", "MIT"],
]);

/**
 * The license `pkg` is under: the one pnpm reported, or, when it reported
 * Unknown, the one read by hand, if every version of it was read.
 */
export const licenseOf = (license, pkg, read = unlabeled) => {
	if (license !== "Unknown") return license;
	const found = new Set(pkg.versions.map((version) => read.get(`${pkg.name}@${version}`)));
	return found.size === 1 && !found.has(undefined) ? [...found][0] : license;
};

// Whether an SPDX expression holds when `ok` says which single licenses are
// acceptable. Handles OR, AND, WITH and parentheses, with AND binding
// tighter, which covers everything npm packages declare.
export const satisfies = (expression, ok) => {
	const tokens = expression.replace(/[()]/g, " $& ").split(/\s+/).filter(Boolean);
	const fail = () => {
		throw new Error(`can't read the license "${expression}"`);
	};
	const keyword = (word) => tokens[i]?.toUpperCase() === word;
	let i = 0;

	const term = () => {
		const token = tokens[i++];
		if (token === "(") {
			const value = or();
			if (tokens[i++] !== ")") fail();
			return value;
		}
		if (token === undefined || token === ")" || /^(AND|OR|WITH)$/i.test(token)) fail();
		if (keyword("WITH")) {
			const exception = tokens[i + 1];
			if (exception === undefined) fail();
			i += 2;
			return ok(`${token} WITH ${exception}`);
		}
		return ok(token);
	};
	const and = () => {
		let value = term();
		while (keyword("AND")) {
			i++;
			value = term() && value;
		}
		return value;
	};
	const or = () => {
		let value = and();
		while (keyword("OR")) {
			i++;
			value = and() || value;
		}
		return value;
	};

	const value = or();
	if (i !== tokens.length) fail();
	return value;
};

const allowedLower = new Set([...allowed].map((id) => id.toLowerCase()));

const main = () => {
	const listing = JSON.parse(
		execFileSync("pnpm", ["-r", "licenses", "list", "--prod", "--json"], { encoding: "utf8" })
	);

	const problems = [];
	let count = 0;
	for (const [license, packages] of Object.entries(listing)) {
		for (const pkg of packages) {
			count++;
			const extra = exceptions.get(pkg.name)?.toLowerCase();
			// SPDX ids match regardless of case, and @pierre/diffs writes apache-2.0.
			const ok = (id) => allowedLower.has(id.toLowerCase()) || id.toLowerCase() === extra;
			let fine = false;
			try {
				fine = satisfies(licenseOf(license, pkg), ok);
			} catch {}
			if (!fine) problems.push(`  ${pkg.name} ${pkg.versions.join(", ")}: ${license}`);
		}
	}

	if (problems.length > 0) {
		console.error("These production packages have licenses that aren't on the allowed list:");
		console.error(problems.join("\n"));
		console.error("Replace them, or record an exception in scripts/check-licenses.mjs.");
		process.exit(1);
	}
	console.log(`All ${count} production npm packages have allowed licenses.`);
};

// Run as a script, not when the tests import from here.
if (import.meta.main) main();
