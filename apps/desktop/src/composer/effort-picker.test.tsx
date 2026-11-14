// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { EFFORT_SINCE, type ModelChoice } from "@maxhq/shared/bindings";
import { account, link } from "@maxhq/shared/test/build";

import { EffortPicker } from "./effort-picker";
import {
	AGENT,
	DEVICE,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	renderComposer,
} from "./testing";

const MODELS: ModelChoice[] = [
	{
		id: "gpt-sol",
		label: "GPT Sol",
		default: true,
		efforts: ["low", "medium", "high"],
		default_effort: "medium",
	},
];

const CLAUDE: ModelChoice[] = [
	{
		id: "opus",
		label: "Opus",
		default: true,
		efforts: ["low", "medium", "high", "xhigh", "max"],
		default_effort: "xhigh",
	},
];

/** Opens the slider and returns its thumb, which takes the focus. */
const openSlider = async () => {
	fireEvent.click(screen.getByRole("button", { name: /^Effort/ }));
	const thumb = await screen.findByRole("slider", { name: "Effort" });
	await waitFor(() => expect(thumb).toHaveFocus());
	return thumb;
};

beforeAll(installBrowserFakes);

describe("EffortPicker", () => {
	it("opens a slider on the agent's level and sends a change", async () => {
		const onPick = vi.fn();
		render(<EffortPicker effort="high" model={null} choices={MODELS} onPick={onPick} />);
		expect(screen.getByRole("button", { name: "Effort: High" })).toHaveTextContent("High");

		const thumb = await openSlider();
		expect(thumb).toHaveAttribute("aria-valuetext", "High");
		const dialog = screen.getByRole("dialog");
		expect(within(dialog).getByText("Default")).toBeInTheDocument();
		fireEvent.keyDown(thumb, { key: "ArrowLeft" });
		expect(onPick).toHaveBeenCalledWith("medium");
		fireEvent.keyDown(thumb, { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("slider")).not.toBeInTheDocument());
	});

	it("names the level a Claude agent runs at when it has none of its own", async () => {
		render(<EffortPicker effort={null} model="opus" choices={CLAUDE} onPick={vi.fn()} />);
		const button = screen.getByRole("button", { name: /^Effort/ });
		expect(button).toHaveTextContent("Extra high");
		expect(button).not.toHaveTextContent(/default/i);
		const thumb = await openSlider();
		expect(thumb).toHaveAttribute("aria-valuetext", "Extra high (default)");
		expect(screen.queryByText(/Vendor default/)).not.toBeInTheDocument();
	});

	it("closes on Enter", async () => {
		render(<EffortPicker effort="low" model={null} choices={MODELS} onPick={vi.fn()} />);
		fireEvent.keyDown(await openSlider(), { key: "Enter" });
		await waitFor(() => expect(screen.queryByRole("slider")).not.toBeInTheDocument());
	});

	it("draws nothing when the model lists no levels", () => {
		const { container } = render(
			<EffortPicker
				effort={null}
				model={null}
				choices={[{ id: "x", label: "X", default: true }]}
				onPick={vi.fn()}
			/>
		);
		expect(container).toBeEmptyDOMElement();
	});
});

describe("the composer's effort picker", () => {
	const codex = account({ id: "acc", vendor: "codex", models: MODELS });
	const agent = heldAgent({ vendor: "codex", account: "acc", effort: "low" });
	const local = (protocol: number) => link({ id: "local", device: DEVICE, protocol });

	it("shows the level and sends a change", async () => {
		const { source } = renderComposer({
			agent,
			account: codex,
			links: [local(EFFORT_SINCE)],
		});
		await findTextbox();
		expect(await screen.findByRole("button", { name: /^Effort/ })).toHaveTextContent("Low");
		fireEvent.keyDown(await openSlider(), { key: "End" });
		await waitFor(() => expect(source.setEffort).toHaveBeenCalledWith(AGENT, "high"));
	});

	it("hides it for a maxd from before effort levels", async () => {
		renderComposer({ agent, account: codex, links: [local(EFFORT_SINCE - 1)] });
		await findTextbox();
		expect(screen.getByRole("button", { name: /^Model/ })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /^Effort/ })).not.toBeInTheDocument();
	});
});
