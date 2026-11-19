// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { GEMINI } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderDevice = (link: string) => {
	const source = mockSource({ now: () => NOW, tick: false });
	const set = vi.spyOn(source, "setAcpAgent");
	const remove = vi.spyOn(source, "deleteAcpAgent");
	const ui = createUiStore();
	ui.getState().setMode("devices");
	ui.getState().selectLink(link);
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { set, remove };
};

const agents = () => screen.findByRole("region", { name: "ACP agents on Mac" });

const form = async () =>
	(await screen.findByRole("form", { name: /^(Change Gemini|New ACP agent)$/ })) as HTMLElement;

describe("ACP agents settings", () => {
	it("lists the Mac's agent with its command and what its last start found", async () => {
		renderDevice("link-mac");
		const section = await agents();
		expect(section).toHaveTextContent("Gemini");
		expect(section).toHaveTextContent("gemini --experimental-acp");
		expect(section).toHaveTextContent(
			"Takes images, keeps history on restart, runs its default model."
		);
	});

	it("shows nothing for a device whose maxd doesn't run them", async () => {
		renderDevice("link-devbox");
		await screen.findByRole("region", { name: "Settings for devbox" });
		expect(screen.queryByRole("region", { name: /^ACP agents/ })).not.toBeInTheDocument();
	});

	it("shows a held value only as set, and sends a replaced one once", async () => {
		const { set } = renderDevice("link-mac");
		fireEvent.click(within(await agents()).getByRole("button", { name: "Edit" }));
		const edit = await form();
		expect(within(edit).getByLabelText("GEMINI_API_KEY is set")).toHaveTextContent("••••••••");
		expect(within(edit).queryByLabelText("Value of GEMINI_API_KEY")).not.toBeInTheDocument();

		fireEvent.click(within(edit).getByRole("button", { name: "Replace GEMINI_API_KEY" }));
		const field = within(edit).getByLabelText("Value of GEMINI_API_KEY") as HTMLInputElement;
		expect(field).toHaveAttribute("type", "password");
		fireEvent.input(field, { target: { value: "new-secret" } });
		fireEvent.click(within(edit).getByRole("button", { name: "Save" }));

		expect(set).toHaveBeenCalledWith(
			"dev-mac",
			expect.objectContaining({ id: GEMINI, env: ["GEMINI_API_KEY"] }),
			[{ name: "GEMINI_API_KEY", value: "new-secret" }]
		);
		expect(field.value).toBe("");
		await waitFor(() => expect(screen.queryByRole("form")).not.toBeInTheDocument());
		expect(document.body.innerHTML).not.toContain("new-secret");
	});

	it("keeps a value when you change your mind about replacing it", async () => {
		const { set } = renderDevice("link-mac");
		fireEvent.click(within(await agents()).getByRole("button", { name: "Edit" }));
		const edit = await form();
		fireEvent.click(within(edit).getByRole("button", { name: "Replace GEMINI_API_KEY" }));
		fireEvent.click(within(edit).getByRole("button", { name: "Keep GEMINI_API_KEY" }));
		fireEvent.click(within(edit).getByRole("button", { name: "Save" }));
		expect(set).toHaveBeenCalledWith(
			"dev-mac",
			expect.objectContaining({ env: ["GEMINI_API_KEY"] }),
			[]
		);
	});

	it("removes a variable with its value", async () => {
		const { set } = renderDevice("link-mac");
		fireEvent.click(within(await agents()).getByRole("button", { name: "Edit" }));
		const edit = await form();
		fireEvent.click(within(edit).getByRole("button", { name: "Remove GEMINI_API_KEY" }));
		fireEvent.click(within(edit).getByRole("button", { name: "Save" }));
		expect(set).toHaveBeenCalledWith("dev-mac", expect.objectContaining({ env: [] }), []);
	});

	it("adds an agent with argument and variable rows", async () => {
		const { set } = renderDevice("link-mac");
		fireEvent.click(within(await agents()).getByRole("button", { name: "Add agent" }));
		const add = await form();
		fireEvent.change(within(add).getByLabelText("Name"), { target: { value: "Goose" } });
		fireEvent.change(within(add).getByLabelText("Program"), { target: { value: "goose" } });
		expect(within(add).getByRole("button", { name: "Browse…" })).toBeInTheDocument();
		fireEvent.click(within(add).getByRole("button", { name: "Add argument" }));
		fireEvent.click(within(add).getByRole("button", { name: "Add argument" }));
		fireEvent.change(within(add).getByLabelText("Argument 1"), { target: { value: "acp" } });
		fireEvent.change(within(add).getByLabelText("Argument 2"), { target: { value: "--x" } });
		fireEvent.click(within(add).getByRole("button", { name: "Remove argument 2" }));
		fireEvent.click(within(add).getByRole("button", { name: "Add variable" }));
		fireEvent.change(within(add).getByLabelText("Variable name"), {
			target: { value: "GOOSE_KEY" },
		});
		fireEvent.input(within(add).getByLabelText("Value of GOOSE_KEY"), {
			target: { value: "k" },
		});
		fireEvent.click(within(add).getByRole("button", { name: "Save" }));

		expect(set).toHaveBeenCalledWith(
			"dev-mac",
			expect.objectContaining({
				device: "dev-mac",
				name: "Goose",
				program: "goose",
				args: ["acp"],
				env: ["GOOSE_KEY"],
				containers: false,
			}),
			[{ name: "GOOSE_KEY", value: "k" }]
		);
		expect(await within(await agents()).findByText("Goose")).toBeInTheDocument();
	});

	it("says what's wrong before it sends anything", async () => {
		const { set } = renderDevice("link-mac");
		fireEvent.click(within(await agents()).getByRole("button", { name: "Add agent" }));
		const add = await form();
		fireEvent.change(within(add).getByLabelText("Name"), { target: { value: "Gemini" } });
		fireEvent.change(within(add).getByLabelText("Program"), { target: { value: "gemini" } });
		fireEvent.click(within(add).getByRole("button", { name: "Save" }));
		expect(within(add).getByRole("status")).toHaveTextContent(
			"Mac already has an agent called Gemini."
		);
		expect(set).not.toHaveBeenCalled();
	});

	it("removes an agent once you confirm", async () => {
		const { remove } = renderDevice("link-mac");
		const section = await agents();
		fireEvent.click(within(section).getByRole("button", { name: "Remove" }));
		fireEvent.click(within(section).getByRole("button", { name: "Remove" }));
		expect(remove).toHaveBeenCalledWith("dev-mac", GEMINI);
		expect(await within(section).findByText("No ACP agents yet")).toBeInTheDocument();
	});
});
