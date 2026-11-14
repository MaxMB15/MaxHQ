// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Bar } from "./usage";

const layers = (container: HTMLElement) => ({
	main: container.querySelector<HTMLElement>('[data-layer="main"]'),
	under: container.querySelector<HTMLElement>('[data-layer="under"]'),
});

describe("Bar", () => {
	it("draws the under figure beneath the main fill, both within the track", () => {
		const { container } = render(<Bar percent={40} under={130} full={false} className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.style.width).toBe("40%");
		expect(under?.style.width).toBe("100%");
		expect(under?.compareDocumentPosition(main!)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
	});

	it("draws one fill without an under figure", () => {
		const { container } = render(<Bar percent={-5} full={false} className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.style.width).toBe("0%");
		expect(under).toBeNull();
	});

	it("keeps the busy color on both layers when full", () => {
		const { container } = render(<Bar percent={100} under={70} full className="h-1" />);
		const { main, under } = layers(container);
		expect(main?.className).toContain("bg-status-busy");
		expect(under?.className).toContain("bg-status-busy");
	});
});
