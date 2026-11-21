// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { account } from "@maxhq/shared/test/build";

import { ModelPicker, PermissionPicker } from "./pickers";

const claude = account({
	id: "acc",
	plan: "max",
	models: [
		{ id: "opus", label: "Opus 4.8", default: true },
		{ id: "sonnet", label: "Sonnet 4.6", default: false },
	],
});

const openMenu = async (name: string | RegExp) => {
	fireEvent.click(screen.getByRole("button", { name }));
	return screen.findByRole("menu");
};

describe("ModelPicker", () => {
	it("lists the account's models and picks one", async () => {
		const onPick = vi.fn();
		render(
			<ModelPicker
				kind="claude"
				model={null}
				account={claude}
				choices={claude.models}
				onPick={onPick}
			/>
		);
		expect(screen.getByRole("button", { name: "Model: Opus 4.8" })).toHaveTextContent(
			"Opus 4.8"
		);

		const menu = await openMenu(/^Model/);
		expect(within(menu).getByText("Model, Claude Max")).toBeInTheDocument();
		const opus = within(menu).getByRole("menuitemradio", { name: /Opus 4\.8/ });
		expect(opus).toHaveAttribute("aria-checked", "true");
		expect(opus).toHaveTextContent("Default");

		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Sonnet 4\.6/ }));
		expect(onPick).toHaveBeenCalledWith("sonnet");
	});

	it("shows the model as plain text when the account lists none", () => {
		render(
			<ModelPicker
				kind="codex"
				model="gpt-5.5"
				account={undefined}
				choices={[]}
				onPick={vi.fn()}
			/>
		);
		expect(screen.queryByRole("button", { name: /^Model/ })).not.toBeInTheDocument();
		expect(screen.getByText("gpt-5.5")).toBeInTheDocument();
	});

	it("draws a smaller chip for the header, named after the model", async () => {
		const onPick = vi.fn();
		render(
			<ModelPicker
				kind="claude"
				model="opus"
				account={claude}
				choices={claude.models}
				place="header"
				onPick={onPick}
			/>
		);
		const menu = await openMenu("Model: Opus 4.8");
		// Picking the model it already runs sends nothing.
		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Opus 4\.8/ }));
		expect(onPick).not.toHaveBeenCalled();
	});

	it("only names the model when there's nothing to pick it with", () => {
		render(
			<ModelPicker
				kind="claude"
				model="opus"
				account={claude}
				choices={claude.models}
				place="header"
			/>
		);
		expect(screen.queryByRole("button")).not.toBeInTheDocument();
		expect(screen.getByText("Opus 4.8")).toBeInTheDocument();
	});

	it("offers the models of other vendors, which move the agent there", async () => {
		const codex = account({
			id: "acc-codex",
			vendor: "codex",
			plan: "plus",
			models: [{ id: "gpt-5.5", label: "GPT-5.5", default: true }],
		});
		const cursor = account({ id: "acc-cursor", vendor: "cursor", plan: "pro" });
		const onPick = vi.fn();
		render(
			<ModelPicker
				kind="claude"
				model="opus"
				account={claude}
				choices={claude.models}
				moves={[
					{ account: codex, models: codex.models },
					{ account: cursor, models: [] },
				]}
				onPick={onPick}
			/>
		);
		const menu = await openMenu(/^Model/);
		const toCodex = within(menu).getByRole("group", { name: "Move to Codex Plus" });
		fireEvent.click(within(toCodex).getByRole("menuitem", { name: "GPT-5.5" }));
		expect(onPick).toHaveBeenLastCalledWith("gpt-5.5", "acc-codex");

		// An account that lists no models yet runs its own default.
		const again = await openMenu(/^Model/);
		const toCursor = within(again).getByRole("group", { name: "Move to Cursor Pro" });
		fireEvent.click(within(toCursor).getByRole("menuitem", { name: "Default model" }));
		expect(onPick).toHaveBeenLastCalledWith("", "acc-cursor");
	});

	it("opens with only other vendors to pick when the account lists no models", async () => {
		const codex = account({ id: "acc-codex", vendor: "codex", plan: "plus" });
		render(
			<ModelPicker
				kind="claude"
				model={null}
				account={{ ...claude, models: [] }}
				choices={[]}
				moves={[{ account: codex, models: [] }]}
				onPick={vi.fn()}
			/>
		);
		const menu = await openMenu(/^Model/);
		expect(within(menu).queryByText("Model, Claude Max")).not.toBeInTheDocument();
		expect(within(menu).getByText("Move to Codex Plus")).toBeInTheDocument();
	});
});

describe("PermissionPicker", () => {
	it("picks a mode", async () => {
		const onPick = vi.fn();
		render(
			<PermissionPicker
				vendor="claude"
				permission={{ requested: "ask", effective: "ask" }}
				onPick={onPick}
			/>
		);
		const menu = await openMenu(/^Permission mode/);
		expect(within(menu).getByRole("menuitemradio", { name: /Ask first/ })).toHaveAttribute(
			"aria-checked",
			"true"
		);
		expect(within(menu).getByRole("menuitemradio", { name: /Plan only/ })).not.toHaveAttribute(
			"aria-disabled",
			"true"
		);

		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Accept edits/ }));
		expect(onPick).toHaveBeenCalledWith("accept_edits");
	});

	it("turns Plan off for Codex", async () => {
		const onPick = vi.fn();
		render(
			<PermissionPicker
				vendor="codex"
				permission={{ requested: "auto", effective: null }}
				onPick={onPick}
			/>
		);
		const menu = await openMenu(/^Permission mode/);
		const plan = within(menu).getByRole("menuitemradio", { name: /Plan only/ });
		expect(plan).toHaveAttribute("aria-disabled", "true");
		expect(plan).toHaveTextContent("Not in Codex");

		fireEvent.click(plan);
		expect(onPick).not.toHaveBeenCalled();
	});

	it("hides Auto for a local ACP agent, and Plan unless it has a plan mode", async () => {
		const modes = async (plan: boolean) => {
			const { unmount } = render(
				<PermissionPicker
					vendor="acp"
					acp={{ plan }}
					permission={{ requested: "ask", effective: null }}
					onPick={vi.fn()}
				/>
			);
			const menu = await openMenu(/^Permission mode/);
			const names = within(menu)
				.getAllByRole("menuitemradio")
				.map((item) => item.textContent);
			unmount();
			return names;
		};
		const without = await modes(false);
		expect(without.join(" ")).not.toMatch(/Auto|Plan only/);
		expect(without).toHaveLength(3);
		const withPlan = await modes(true);
		expect(withPlan).toHaveLength(4);
		expect(withPlan.join(" ")).toContain("Plan only");
		expect(withPlan.join(" ")).not.toContain("Not in Codex");
	});

	it("shows the mode the agent runs in when it differs from the one asked for", async () => {
		render(
			<PermissionPicker
				vendor="claude"
				permission={{ requested: "full_access", effective: "ask" }}
				onPick={vi.fn()}
			/>
		);
		expect(
			screen.getByRole("button", { name: "Permission mode: Ask first" })
		).toHaveTextContent("Ask first");

		const menu = await openMenu(/^Permission mode/);
		expect(within(menu).getByRole("menuitemradio", { name: /Ask first/ })).toHaveAttribute(
			"aria-checked",
			"true"
		);
		expect(menu).toHaveTextContent("Runs as Ask first. You asked for Full access.");
	});

	it("says nothing extra once the agent runs in the mode asked for", async () => {
		render(
			<PermissionPicker
				vendor="claude"
				permission={{ requested: "plan", effective: "plan" }}
				onPick={vi.fn()}
			/>
		);
		const menu = await openMenu(/^Permission mode/);
		expect(menu).not.toHaveTextContent("Runs as");
	});
});

describe("in a narrow footer", () => {
	it("names the model's id in a tooltip when its name says less", async () => {
		render(
			<ModelPicker
				kind="claude"
				model="claude-opus-4-8"
				account={claude}
				choices={claude.models}
				onPick={vi.fn()}
			/>
		);
		const trigger = screen.getByRole("button", { name: "Model: Opus 4.8" });
		expect(trigger).toHaveTextContent("Opus 4.8");
		act(() => trigger.focus());
		expect(await screen.findByText("claude-opus-4-8")).toBeInTheDocument();
	});

	it("cuts the model's name short, and keeps it whole in its name and tooltip", async () => {
		render(
			<ModelPicker
				kind="claude"
				model="sonnet"
				account={claude}
				choices={claude.models}
				folded
				onPick={vi.fn()}
			/>
		);
		const trigger = screen.getByRole("button", { name: "Model: Sonnet 4.6" });
		expect(within(trigger).getByText("Sonnet 4.6")).toHaveClass("truncate", "min-w-[4ch]");
		act(() => trigger.focus());
		expect(await screen.findAllByText("Sonnet 4.6")).toHaveLength(2);
	});

	it("keeps a name too short to cut whole", () => {
		const short = [{ id: "o3", label: "o3", default: true }];
		render(
			<ModelPicker
				kind="codex"
				model="o3"
				account={claude}
				choices={short}
				folded
				onPick={vi.fn()}
			/>
		);
		expect(
			within(screen.getByRole("button", { name: "Model: o3" })).getByText("o3")
		).not.toHaveClass("min-w-[4ch]");
	});

	it("folds the permission mode to its icon, and keeps it in its name and tooltip", async () => {
		const onPick = vi.fn();
		render(
			<PermissionPicker
				vendor="claude"
				permission={{ requested: "ask", effective: null }}
				folded
				onPick={onPick}
			/>
		);
		const trigger = screen.getByRole("button", { name: "Permission mode: Ask first" });
		expect(trigger).toHaveTextContent(/^$/);
		act(() => trigger.focus());
		expect(await screen.findByText("Permission mode: Ask first")).toBeInTheDocument();

		const menu = await openMenu(/^Permission mode/);
		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Plan/ }));
		expect(onPick).toHaveBeenCalledWith("plan");
	});
});
