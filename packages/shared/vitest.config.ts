// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { defineConfig } from "vitest/config";

export default defineConfig({
	test: {
		// Node has no DOM, like React Native, so a test that needs one fails.
		environment: "node",
		include: ["src/**/*.test.ts"],
		globals: false,
	},
});
