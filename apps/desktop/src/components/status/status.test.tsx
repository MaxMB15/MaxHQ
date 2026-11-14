// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ACTIVITY_LABEL, type ActivityLook } from "@maxhq/shared/status/activity";
import type { Check } from "@maxhq/shared/status/card";
import { CONNECTION_LABEL, type ConnectionLook } from "@maxhq/shared/status/connection";

import { ActivityDot, ConnectionDot } from "./dots";
import { checksSummary, HoverCardBody } from "./status-hover";

const ACTIVITY_LOOKS = Object.keys(ACTIVITY_LABEL) as ActivityLook[];
const CONNECTION_LOOKS = Object.keys(CONNECTION_LABEL) as ConnectionLook[];

describe("status dots", () => {
	it.each(ACTIVITY_LOOKS)("draws the %s activity dot with its name", (look) => {
		render(<ActivityDot look={look} />);
		const dot = screen.getByRole("img", { name: ACTIVITY_LABEL[look] });
		expect(dot).toHaveAttribute("data-look", look);
	});

	it.each(CONNECTION_LOOKS)("draws the %s connection dot with its name", (look) => {
		render(<ConnectionDot look={look} size={8} />);
		const dot = screen.getByRole("img", { name: CONNECTION_LABEL[look] });
		expect(dot).toHaveAttribute("data-look", look);
	});

	it("gives every state its own name", () => {
		expect(new Set(Object.values(ACTIVITY_LABEL)).size).toBe(ACTIVITY_LOOKS.length);
		expect(new Set(Object.values(CONNECTION_LABEL)).size).toBe(CONNECTION_LOOKS.length);
	});

	it("hides a decorative dot and takes a label in place of the state's name", () => {
		const { container } = render(
			<>
				<ActivityDot look="busy" decorative />
				<ConnectionDot
					look="agent_down"
					label="Worst agent: device up, agent not running"
				/>
			</>
		);
		expect(screen.queryByRole("img", { name: "Busy" })).not.toBeInTheDocument();
		expect(container.querySelector('[data-look="busy"]')).toHaveAttribute(
			"aria-hidden",
			"true"
		);
		expect(
			screen.getByRole("img", { name: "Worst agent: device up, agent not running" })
		).toBeInTheDocument();
	});
});

const check = (short: string, state: Check["state"]): Check => ({
	id: "ssh",
	short,
	title: short,
	state,
	detail: "",
});

describe("status hover", () => {
	it("sums up the checks for a screen reader", () => {
		expect(
			checksSummary([
				check("Net", "passed"),
				check("Tailnet", "passed"),
				check("Peer", "passed"),
				check("SSH", "running"),
				check("maxd", "not_run"),
				check("Agent", "unknown"),
			])
		).toBe(
			"Checks: Net, Tailnet and Peer passed, SSH in progress, maxd not run, Agent not checked"
		);
		expect(checksSummary([check("Peer", "failed")])).toBe("Checks: Peer failed");
	});

	it("lays out the card's title, body, checks, facts and foot", () => {
		render(
			<HoverCardBody
				dot={<ConnectionDot look="connecting" decorative />}
				card={{
					title: "Reconnecting to devbox",
					body: "SSH is connecting.",
					checks: [check("Peer", "passed"), check("SSH", "running")],
					facts: [{ label: "Route", value: "SSH as dev@devbox" }],
					foot: "Second try",
				}}
			/>
		);
		expect(screen.getByText("Reconnecting to devbox")).toBeInTheDocument();
		expect(screen.getByText("SSH is connecting.")).toBeInTheDocument();
		expect(
			screen.getByRole("img", { name: "Checks: Peer passed, SSH in progress" })
		).toBeInTheDocument();
		expect(screen.getByRole("term")).toHaveTextContent("Route");
		expect(screen.getByRole("definition")).toHaveTextContent("SSH as dev@devbox");
		expect(screen.getByText("Second try")).toBeInTheDocument();
	});
});
