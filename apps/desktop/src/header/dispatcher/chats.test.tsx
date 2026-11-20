// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { type ReactNode, useEffect, useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { ComposerNativeProvider } from "@/composer/native";
import { editorOf, fakeNative, installBrowserFakes } from "@/composer/testing";
import { mockSource } from "@/data/mock-source";
import { panesOf } from "@/layout/panes";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { AgentId } from "@maxhq/shared/bindings";
import { NOTES_PROJECT } from "@maxhq/shared/data/mock-notes";
import { MAC } from "@maxhq/shared/data/mock-view";
import { createDispatchFlow } from "@maxhq/shared/dispatch/flow";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { DispatchBox } from "../dispatcher";
import { MovedBar } from "./chats";

beforeAll(installBrowserFakes);

afterEach(() => {
	localStorage.clear();
	clearKept();
});

/** Hands the services out once they're there. */
const Grab = ({ onServices }: { onServices: (services: Services) => void }) => {
	const services = useServices();
	useEffect(() => {
		onServices(services);
	}, [onServices, services]);
	return null;
};

/** `children` with the sample source, once the view has the Mac's chats. */
const renderWith = async (children: ReactNode) => {
	const held: { services: Services | null } = { services: null };
	render(
		<FixedNow value={NOW}>
			<ServicesProvider
				source={mockSource({ now: () => NOW, tick: false })}
				ui={createUiStore()}
			>
				<ComposerNativeProvider value={fakeNative().native}>
					<Grab
						onServices={(services) => {
							held.services = services;
						}}
					/>
					{children}
					<ConfirmHost />
				</ComposerNativeProvider>
			</ServicesProvider>
		</FixedNow>
	);
	await waitFor(() =>
		expect(held.services?.view.getState().view.tables.dispatcher_chat.size).toBeGreaterThan(0)
	);
	const services = held.services!;
	return { services, view: () => services.view.getState().view };
};

/** The box after one request, with the chats listed in its panel. */
const listChats = async () => {
	const shown = await renderWith(<DispatchBox />);
	const input = screen.getByRole("textbox", { name: "Ask the dispatcher" });
	fireEvent.focus(input);
	act(() => {
		editorOf(input).commands.insertContent("Hello");
	});
	fireEvent.keyDown(input, { key: "Enter" });
	const panel = within(await screen.findByRole("region", { name: "Dispatcher" }));
	fireEvent.click(await panel.findByRole("button", { name: "Chats" }));
	return { ...shown, panel, search: panel.getByRole("searchbox", { name: "Search chats" }) };
};

/** The chats under a heading, by title. */
const titlesIn = (group: string) =>
	within(screen.getByRole("region", { name: group }))
		.getAllByRole("listitem")
		.map((row) => row.querySelector("button span span")?.textContent);

/** A chat's row, by its title. */
const rowOf = (title: string) =>
	within(screen.getByRole("button", { name: new RegExp(`^${title}`) }).closest("li")!);

describe("the dispatcher's chats", () => {
	it("lists the chats by group, and search narrows them to the matches", async () => {
		const { panel, search } = await listChats();
		expect(search).toHaveFocus();
		expect(titlesIn("Today")).toEqual(["Chat 6", "Slow login page"]);
		expect(titlesIn("This week")).toEqual([
			"Release notes for the next version",
			"Flaky Linux tests",
		]);
		expect(titlesIn("Archived")).toEqual(["Terminal colors", "Picking a chart library"]);
		expect(screen.queryByRole("region", { name: "Earlier" })).not.toBeInTheDocument();

		fireEvent.change(search, { target: { value: "linux runner" } });
		expect(titlesIn("Matches")).toEqual(["Flaky Linux tests"]);
		fireEvent.change(search, { target: { value: "nothing like it" } });
		expect(panel.getByText("No chats match")).toBeInTheDocument();

		// Esc clears the search, then closes the list.
		fireEvent.keyDown(search, { key: "Escape" });
		expect(search).toHaveValue("");
		expect(titlesIn("Today")).toHaveLength(2);
		fireEvent.keyDown(search, { key: "Escape" });
		expect(panel.queryByRole("searchbox")).not.toBeInTheDocument();
		expect(panel.getByRole("log", { name: "Conversation" })).toBeInTheDocument();
	});

	it("makes the chat you pick the one you're in", async () => {
		const { panel, view } = await listChats();
		fireEvent.click(panel.getByRole("button", { name: /^Slow login page/ }));
		expect(
			await panel.findByText("Why does the login page take ten seconds on devbox?")
		).toBeInTheDocument();
		expect(view().tables.dispatcher.get(MAC)?.agent).toBe("agent-chat-login");
		expect(panel.queryByRole("searchbox")).not.toBeInTheDocument();

		// The one you're in shows as such.
		fireEvent.click(panel.getByRole("button", { name: "Chats" }));
		expect(panel.getByRole("button", { name: /^Slow login page/ })).toHaveAttribute(
			"aria-current",
			"true"
		);
	});

	it("renames and archives a chat, and deletes it once you confirm", async () => {
		const { panel, view } = await listChats();
		fireEvent.click(rowOf("Flaky Linux tests").getByRole("button", { name: "Rename" }));
		const field = panel.getByRole("textbox", { name: "Rename chat" });
		fireEvent.change(field, { target: { value: "CI flakes" } });
		fireEvent.keyDown(field, { key: "Enter" });
		await waitFor(() => expect(titlesIn("This week")).toContain("CI flakes"));
		expect(view().tables.dispatcher_chat.get("agent-chat-ci")?.title).toBe("CI flakes");

		fireEvent.click(rowOf("CI flakes").getByRole("button", { name: "Archive" }));
		await waitFor(() => expect(titlesIn("Archived")).toContain("CI flakes"));
		expect(rowOf("CI flakes").getByRole("button", { name: "Unarchive" })).toBeInTheDocument();

		fireEvent.click(rowOf("CI flakes").getByRole("button", { name: "Delete" }));
		const dialog = within(await screen.findByRole("alertdialog"));
		expect(dialog.getByText('Delete chat "CI flakes"?')).toBeInTheDocument();
		const confirm = dialog.getByRole("button", { name: "Delete" });
		// A press in the question leaves the panel open.
		fireEvent.pointerDown(confirm);
		fireEvent.click(confirm);
		await waitFor(() => expect(view().tables.dispatcher_chat.has("agent-chat-ci")).toBe(false));
		expect(panel.getByRole("searchbox", { name: "Search chats" })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /^CI flakes/ })).not.toBeInTheDocument();
	});

	it("saves a chat to Notes and opens the note it wrote", async () => {
		const { services, view } = await listChats();
		const opened = vi.spyOn(panesOf(services), "openNote");
		fireEvent.click(rowOf("Slow login page").getByRole("button", { name: "Save to Notes" }));
		await waitFor(() => expect(opened).toHaveBeenCalled());
		const [project, note] = opened.mock.calls[0]!;
		expect(project).toBe(NOTES_PROJECT);
		const saved = [...view().tables.project_notes.values()]
			.find((n) => n.device === MAC && n.project === project)
			?.notes.find((n) => n.note === note);
		expect(saved).toMatchObject({
			title: "Slow login page",
			by: { agent: "agent-chat-login" },
		});
		expect(services.ui.getState().mode).toBe("projects");
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();
	});
});

/** The bar in the chat `current`, with a flow of its own. */
const Moved = ({ current }: { current: AgentId }) => {
	const services = useServices();
	const [flow] = useState(() => createDispatchFlow(services.source));
	return <MovedBar flow={flow} device={MAC} current={current} />;
};

describe("the moved bar", () => {
	it("says where the dispatcher moved your message, and keeps it here", async () => {
		const { view } = await renderWith(<Moved current="agent-chat-release" />);
		const bar = within(await screen.findByRole("status"));
		expect(bar.getByText("Moved your message to Slow login page")).toBeInTheDocument();
		fireEvent.click(bar.getByRole("button", { name: "Keep here" }));
		await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
		expect(view().tables.dispatcher.get(MAC)?.agent).toBe("agent-chat-release");
		expect(view().tables.dispatcher_chat.get("agent-chat-release")?.moved_to).toBeNull();
	});

	it("says where your message came from in the chat it went to", async () => {
		await renderWith(<Moved current="agent-chat-login" />);
		const bar = within(await screen.findByRole("status"));
		expect(
			bar.getByText("Moved your message here from Release notes for the next version")
		).toBeInTheDocument();
		expect(bar.getByRole("button", { name: "Move back" })).toBeInTheDocument();
	});
});
