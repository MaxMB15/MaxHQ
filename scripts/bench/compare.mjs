// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Compares the latest criterion run against a saved baseline and fails when
// any benchmark got more than 15% slower. Criterion prints the change itself
// but never fails, so this is what turns a regression into an exit code.
//
// Run both sides on the same machine. Shared CI runners vary by more than 15%
// from run to run, so CI doesn't run the benchmarks.
//
//   cargo bench --workspace --bench '*' -- --save-baseline main      # on main
//   cargo bench --workspace --bench '*' -- --baseline-lenient main   # on the branch
//   node scripts/bench/compare.mjs main

import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const LIMIT = 0.15;

const baseline = process.argv[2];
if (!baseline) {
	console.error("usage: node scripts/bench/compare.mjs BASELINE");
	process.exit(2);
}

const root = path.resolve(import.meta.dirname, "../../target/criterion");
if (!existsSync(root)) {
	console.error(`${root} doesn't exist. Run cargo bench first.`);
	process.exit(2);
}

// Every folder holding a `new` run is one benchmark, however deeply criterion
// nested its group and parameter names.
const benchmarks = [];
const walk = (dir) => {
	const entries = readdirSync(dir, { withFileTypes: true });
	if (entries.some((entry) => entry.isDirectory() && entry.name === "new")) {
		benchmarks.push(dir);
		return;
	}
	for (const entry of entries) {
		if (entry.isDirectory() && entry.name !== "report") walk(path.join(dir, entry.name));
	}
};
walk(root);

const mean = (dir, run) =>
	JSON.parse(readFileSync(path.join(dir, run, "estimates.json"), "utf8")).mean.point_estimate;

const rows = [];
let slower = 0;
let missing = 0;
for (const dir of benchmarks.sort()) {
	const name = path.relative(root, dir);
	if (!existsSync(path.join(dir, baseline, "estimates.json"))) {
		missing++;
		rows.push(`  ${name}: no ${baseline} run to compare against`);
		continue;
	}
	const change = mean(dir, "new") / mean(dir, baseline) - 1;
	const verdict = change > LIMIT ? "SLOWER" : "ok";
	if (change > LIMIT) slower++;
	rows.push(`  ${name}: ${change >= 0 ? "+" : ""}${(change * 100).toFixed(1)}% ${verdict}`);
}

console.log(`Mean time against ${baseline}, failing above +${LIMIT * 100}%:`);
console.log(rows.join("\n"));
if (benchmarks.length === 0 || missing === benchmarks.length) {
	console.error(`No benchmark has a ${baseline} run.`);
	process.exit(2);
}
if (slower > 0) {
	console.error(
		`${slower} benchmark${slower === 1 ? "" : "s"} got more than ${LIMIT * 100}% slower.`
	);
	process.exit(1);
}
