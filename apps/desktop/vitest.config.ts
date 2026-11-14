// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
	plugins: [react()],
	resolve: {
		alias: {
			"@": path.resolve(import.meta.dirname, "./src"),
		},
	},
	test: {
		environment: "jsdom",
		// Each worker loads jsdom once, and every test file still gets its own
		// VM context and window, so files stay isolated and the suite runs
		// faster. Globals Vitest copies in from Node, such as structuredClone
		// and fetch, come from another realm, so an `instanceof` check on what
		// they return can fail.
		pool: "vmThreads",
		// A long flow can pass five seconds on a busy machine, which fails the
		// commit hook for nothing. A real hang still fails.
		testTimeout: 20_000,
		setupFiles: ["./src/test/setup.ts"],
		include: ["src/**/*.{test,spec}.{ts,tsx}"],
		globals: false,
		// CSS imports come in empty, except the theme, so a test can read
		// its tokens with `?raw`, and the pane sheets' text read with `?raw`.
		css: {
			include: [
				/styles\/globals\.css/,
				/main-area\/panes\.css\?raw$/,
				/dockview-react\/dist\/styles\/dockview\.css\?raw$/,
			],
		},
		// Node can't load khroma's imports, which Mermaid needs, on its own.
		server: { deps: { inline: ["mermaid", "khroma"] } },
		coverage: {
			provider: "v8",
			include: ["src/**/*.{ts,tsx}"],
			exclude: [
				"src/**/*.{test,spec}.{ts,tsx}",
				"src/test/**",
				"src/vite-env.d.ts",
				"src/main.tsx",
			],
			reporter: ["text", "text-summary", "html", "json-summary"],
			reportsDirectory: "./coverage",
		},
	},
});
