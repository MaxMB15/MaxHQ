// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import assert from "node:assert/strict";
import { test } from "node:test";
import { satisfies } from "./check-licenses.mjs";

const ok = (id) => ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception"].includes(id);

test("a single license", () => {
	assert.equal(satisfies("MIT", ok), true);
	assert.equal(satisfies("GPL-3.0-only", ok), false);
});

test("OR needs one side and AND needs both", () => {
	assert.equal(satisfies("GPL-3.0-only OR MIT", ok), true);
	assert.equal(satisfies("MIT AND GPL-3.0-only", ok), false);
	assert.equal(satisfies("MIT AND Apache-2.0", ok), true);
});

test("AND binds tighter than OR", () => {
	assert.equal(satisfies("GPL-3.0-only AND MIT OR Apache-2.0", ok), true);
	assert.equal(satisfies("MIT OR GPL-3.0-only AND EPL-2.0", ok), true);
	assert.equal(satisfies("GPL-3.0-only OR EPL-2.0 AND MIT", ok), false);
});

test("parentheses and WITH", () => {
	assert.equal(satisfies("(MIT OR GPL-3.0-only) AND Apache-2.0", ok), true);
	assert.equal(satisfies("(GPL-3.0-only OR EPL-2.0) AND MIT", ok), false);
	assert.equal(satisfies("Apache-2.0 WITH LLVM-exception", ok), true);
	assert.equal(satisfies("GPL-2.0-only WITH Classpath-exception-2.0", ok), false);
});

test("lowercase keywords, as some packages write them", () => {
	assert.equal(satisfies("GPL-3.0-only or MIT", ok), true);
});

test("anything it can't parse throws", () => {
	for (const bad of ["", "MIT OR", "(MIT", "MIT)", "AND MIT", "MIT WITH"]) {
		assert.throws(() => satisfies(bad, ok), /can't read/, bad);
	}
});
