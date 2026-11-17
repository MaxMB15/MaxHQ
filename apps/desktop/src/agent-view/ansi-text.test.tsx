// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { AnsiLines, ansiLines } from "./ansi-text";

describe("AnsiLines", () => {
	it("draws the 16 colours with the theme's terminal tokens", () => {
		const { container } = render(
			<pre>
				<AnsiLines lines={ansiLines("\x1b[31mfailed\x1b[0m \x1b[92;44mok\x1b[0m")} />
			</pre>
		);
		const [failed, ok] = container.querySelectorAll("span");
		expect(failed?.style.color).toBe("var(--terminal-red)");
		expect(ok?.style.color).toBe("var(--terminal-bright-green)");
		expect(ok?.style.backgroundColor).toBe("var(--terminal-blue)");
	});

	it("keeps colours past the 16 as they are", () => {
		const { container } = render(
			<pre>
				<AnsiLines lines={ansiLines("\x1b[38;2;1;2;3mdeep\x1b[0m")} />
			</pre>
		);
		expect(container.querySelector("span")?.style.color).toBe("rgb(1, 2, 3)");
	});
});
