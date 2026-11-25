// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { Dialog } from "@base-ui/react/dialog";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Option } from "@maxhq/shared/new-agent/spec";

import { Picker } from "./picker";

// An OpenRouter login lists hundreds of models; twenty is enough for the field.
const routed: Option[] = [
	{ value: "anthropic/z-ai/glm-5.3-flash[1m]", label: "Z.ai: GLM 5.3 Flash" },
	{ value: "anthropic/z-ai/glm-5.3", label: "Z.ai: GLM 5.3" },
	{ value: "anthropic/openai/gpt-5.5", label: "OpenAI: GPT-5.5", detail: "The default" },
	...Array.from({ length: 17 }, (_, i) => ({
		value: `anthropic/vendor/model-${i}`,
		label: `Vendor: Model ${i}`,
	})),
];

// The chip is a combobox, as the plain picker is, so the form finds it the same way.
const trigger = () => screen.getByRole("combobox", { name: /^Model,/ });
const field = () => screen.getByRole("combobox", { name: "Search models" });
const rows = () => within(screen.getByRole("listbox")).queryAllByRole("option");

// It opens with the field focused, a frame after the click.
const open = async () => {
	fireEvent.click(trigger());
	await screen.findByRole("listbox");
	await vi.waitFor(() => expect(field()).toHaveFocus());
};

describe("the model picker with a long list", () => {
	it("opens with a search field that narrows the models by every word typed", async () => {
		const onChange = vi.fn();
		render(
			<Picker
				label="Model"
				models
				value="anthropic/openai/gpt-5.5"
				options={routed}
				onChange={onChange}
			/>
		);
		expect(trigger()).toHaveAccessibleName("Model, OpenAI: GPT-5.5");
		await open();
		expect(rows()).toHaveLength(20);

		fireEvent.change(field(), { target: { value: "glm 5.3 flash" } });
		expect(rows().map((r) => r.textContent)).toEqual(["Z.ai: GLM 5.3 Flash"]);
		fireEvent.change(field(), { target: { value: "z-ai/glm" } });
		expect(rows()).toHaveLength(2);

		fireEvent.click(screen.getByRole("option", { name: "Z.ai: GLM 5.3 Flash" }));
		expect(onChange).toHaveBeenCalledWith("anthropic/z-ai/glm-5.3-flash[1m]");
	});

	it("ticks the model it picks", async () => {
		render(
			<Picker
				label="Model"
				models
				value="anthropic/openai/gpt-5.5"
				options={routed}
				onChange={vi.fn()}
			/>
		);
		await open();
		expect(screen.getByRole("option", { name: /GPT-5\.5/ })).toHaveAttribute(
			"aria-selected",
			"true"
		);
	});

	it("says when no model matches", async () => {
		render(<Picker label="Model" models value="" options={routed} onChange={vi.fn()} />);
		await open();
		fireEvent.change(field(), { target: { value: "nothing like it" } });
		expect(screen.getByRole("status")).toHaveTextContent("No models match");
		expect(rows()).toHaveLength(0);
	});

	it("keeps the setup row while searching local models", async () => {
		const local: Option[] = [
			...routed.map((o) => ({ ...o, group: "This Mac" })),
			{ value: "local-setup", label: "Set up local models…" },
		];
		render(<Picker label="Model" models value="" options={local} onChange={vi.fn()} />);
		await open();
		fireEvent.change(field(), { target: { value: "nothing like it" } });
		expect(screen.getByRole("status")).toHaveTextContent("No models match");
		expect(rows().map((r) => r.textContent)).toEqual(["Set up local models…"]);
	});

	it("moves through the rows with the arrows, picks with Enter, and closes with Esc", async () => {
		const onChange = vi.fn();
		render(<Picker label="Model" models value="" options={routed} onChange={onChange} />);
		await open();
		fireEvent.change(field(), { target: { value: "z.ai" } });
		fireEvent.keyDown(field(), { key: "ArrowDown" });
		fireEvent.keyDown(field(), { key: "ArrowDown" });
		await vi.waitFor(() =>
			expect(screen.getByRole("option", { name: "Z.ai: GLM 5.3" })).toHaveAttribute(
				"data-highlighted"
			)
		);
		expect(field()).toHaveFocus();
		fireEvent.keyDown(field(), { key: "Enter" });
		expect(onChange).toHaveBeenCalledWith("anthropic/z-ai/glm-5.3");
		await vi.waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());

		// It opens again with the field empty.
		await open();
		expect(field()).toHaveValue("");
		expect(rows()).toHaveLength(20);
		fireEvent.change(field(), { target: { value: "v" } });
		fireEvent.keyDown(field(), { key: "Escape" });
		await vi.waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
		await open();
		expect(field()).toHaveValue("");
	});

	it("closes only its list on Esc inside the new agent form", async () => {
		const onOpenChange = vi.fn();
		render(
			<Dialog.Root open onOpenChange={onOpenChange}>
				<Dialog.Portal>
					<Dialog.Popup aria-label="New agent">
						<Picker label="Model" models value="" options={routed} onChange={vi.fn()} />
					</Dialog.Popup>
				</Dialog.Portal>
			</Dialog.Root>
		);
		await open();
		fireEvent.keyDown(field(), { key: "Escape" });
		await vi.waitFor(() => expect(screen.queryByRole("listbox")).not.toBeInTheDocument());
		expect(onOpenChange).not.toHaveBeenCalled();
		expect(screen.getByRole("dialog", { name: "New agent" })).toBeInTheDocument();
	});

	it("stays a plain list for a short one, and for pickers of other things", async () => {
		const short = routed.slice(0, 3);
		const { unmount } = render(
			<Picker label="Model" models value="" options={short} onChange={vi.fn()} />
		);
		fireEvent.click(screen.getByRole("combobox", { name: /^Model,/ }));
		await screen.findByRole("listbox");
		expect(screen.queryByRole("combobox", { name: "Search models" })).not.toBeInTheDocument();
		unmount();

		render(<Picker label="Project" value="" options={routed} onChange={vi.fn()} />);
		fireEvent.click(screen.getByRole("combobox", { name: /^Project,/ }));
		await screen.findByRole("listbox");
		expect(screen.queryByRole("combobox", { name: "Search models" })).not.toBeInTheDocument();
	});
});
