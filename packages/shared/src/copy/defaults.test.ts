// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import {
	COPY_MAX_BYTES,
	COPY_MAX_FILES,
	COPY_WARN_BYTES,
	DEFAULT_COPY_MAX_BYTES,
	copySize,
	defaultTicks,
	isDefaultCopied,
} from "./defaults";

const file = (path: string, size = 100) => ({ path, size });

describe("copy defaults", () => {
	it("ticks env files wherever they sit, and nothing else", () => {
		const files = [
			file(".env"),
			file(".env.local"),
			file("apps/web/.env.development.local"),
			file(".envrc"),
			file("apps/api/.dev.vars"),
			file("db/dev.sqlite3"),
			file("localhost-key.pem"),
			file("env.ts"),
			file(".environment"),
		];
		expect([...defaultTicks(files)]).toEqual([
			".env",
			".env.local",
			"apps/web/.env.development.local",
			".envrc",
			"apps/api/.dev.vars",
		]);
	});

	it("leaves an env file over 1 MiB for you to tick", () => {
		expect(isDefaultCopied(file(".env", DEFAULT_COPY_MAX_BYTES))).toBe(true);
		expect(isDefaultCopied(file(".env", DEFAULT_COPY_MAX_BYTES + 1))).toBe(false);
	});

	it("adds up only the ticked files", () => {
		const files = [file(".env", 1_204), file(".envrc", 92), file("db/seed.sql", 186_413_056)];
		expect(copySize(files, new Set([".env", ".envrc"]))).toEqual({
			files: 2,
			bytes: 1_296,
			look: "fine",
		});
		expect(copySize(files, new Set()).files).toBe(0);
	});

	it("warns past the warning size and refuses past one transfer's bytes", () => {
		const at = (size: number) =>
			copySize([file("db/dump.sql", size)], new Set(["db/dump.sql"]));
		expect(at(COPY_WARN_BYTES).look).toBe("fine");
		expect(at(COPY_WARN_BYTES + 1).look).toBe("large");
		expect(at(COPY_MAX_BYTES).look).toBe("large");
		expect(at(COPY_MAX_BYTES + 1).look).toBe("too_large");
	});

	it("counts the folders the files sit in against one transfer's entries", () => {
		// The transfer's top folder and logs/ take two entries.
		const fits = Array.from({ length: COPY_MAX_FILES - 2 }, (_, i) => file(`logs/${i}.log`, 1));
		const ticked = (files: { path: string }[]) => new Set(files.map((f) => f.path));
		expect(copySize(fits, ticked(fits)).look).toBe("fine");
		const over = [...fits, file("logs/extra.log", 1)];
		expect(copySize(over, ticked(over)).look).toBe("too_large");
		const nested = [file("a/b/c/one.log", 1), file("a/b/two.log", 1), file("three.log", 1)];
		expect(copySize(nested, ticked(nested)).files).toBe(3);
	});
});
