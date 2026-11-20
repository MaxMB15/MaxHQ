// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Row, Section, StatusNote } from "./controls";

describe("a settings row", () => {
	it("lets what it shows be selected, but not its name", () => {
		render(
			<Section title="Network access">
				<Row title="Addresses for phones" sub="192.168.1.30, devbox.example.net">
					<StatusNote note={{ text: "Port 7466 is in use.", error: true }} />
				</Row>
			</Section>
		);
		expect(screen.getByText("192.168.1.30, devbox.example.net")).toHaveClass("selectable");
		expect(screen.getByRole("status")).toHaveClass("selectable");
		expect(screen.getByText("Addresses for phones").closest(".selectable")).toBeNull();
		expect(screen.getByRole("heading", { name: "Network access" })).not.toHaveClass(
			"selectable"
		);
	});
});
