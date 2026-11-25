// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { rowKind, rowName } from "./context";

describe("rowName", () => {
	it("calls the agent by the name the view gives, such as a cloud service's", () => {
		expect(rowName({ vendor: "claude", kind: "local", name: "OpenRouter" })).toBe("OpenRouter");
	});

	it("falls back to the kind's name, then the vendor's", () => {
		expect(rowName({ vendor: "claude", kind: "local" })).toBe("Local");
		expect(rowName({ vendor: "codex" })).toBe("Codex");
		expect(rowKind({ vendor: "cursor" })).toBe("cursor");
	});
});
