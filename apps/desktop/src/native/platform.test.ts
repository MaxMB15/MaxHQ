// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { afterEach, describe, expect, it, vi } from "vitest";
import { appOs, onLinux, onMac, onWindows } from "./platform";

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15";
const WINDOWS =
	"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0.0.0 Edg/131.0.0.0";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 Version/17.0 Safari/605.1.15";

afterEach(() => {
	vi.restoreAllMocks();
});

describe("appOs", () => {
	it("names the system from the web view's user agent", () => {
		const agent = vi.spyOn(navigator, "userAgent", "get");
		agent.mockReturnValue(MAC);
		expect(appOs()).toBe("macos");
		expect([onMac(), onWindows(), onLinux()]).toEqual([true, false, false]);
		agent.mockReturnValue(WINDOWS);
		expect(appOs()).toBe("windows");
		expect([onMac(), onWindows(), onLinux()]).toEqual([false, true, false]);
		agent.mockReturnValue(LINUX);
		expect(appOs()).toBe("linux");
		expect([onMac(), onWindows(), onLinux()]).toEqual([false, false, true]);
	});
	it("reads the test runner's own user agent as Linux, as the Mac-only paths expect", () => {
		expect(navigator.userAgent).not.toContain("Mac");
		expect(appOs()).toBe("linux");
	});
});
