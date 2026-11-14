// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The tests cover the screens' logic, which imports no React Native, and the
// build scripts.

import { defineConfig } from "vitest/config";

export default defineConfig({
	test: { environment: "node", include: ["src/**/*.test.ts", "scripts/**/*.test.ts"] },
});
