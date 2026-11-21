// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Markdown } from "@/agent-view/markdown";
import { mockSource } from "@/data/mock-source";
import { panesOf } from "@/layout/panes";
import { settingsOf } from "@/settings/store";
import { DEVBOX, DEVBOX_LINK } from "@maxhq/shared/data/mock-view";
import { createUiStore } from "@maxhq/shared/ui/store";
import { all } from "@maxhq/shared/view/replica";

import { type Services, ServicesProvider, useServices } from "./services";

/** A dispatcher's answer with these links, and the window's services. */
const answer = async (links: string) => {
	let services: Services | undefined;
	const Grab = () => {
		services = useServices();
		return null;
	};
	render(
		<ServicesProvider
			source={mockSource({ tick: false, scenario: "connected" })}
			ui={createUiStore()}
		>
			<Grab />
			<Markdown text={links} />
		</ServicesProvider>
	);
	await waitFor(() => expect(all(services!.view.getState().view, "device_link")).not.toEqual([]));
	return services!;
};

const click = (name: string) => fireEvent.click(screen.getByRole("link", { name }));

describe("a link to a view of the app", () => {
	it("opens where an old Settings page went, a sign-in or a project's tab", async () => {
		const services = await answer(
			"[Editor](maxhq://settings/editor) [Codex](maxhq://settings/codex) " +
				"[Skills](maxhq://project/p1/skills) [Last](maxhq://settings)"
		);
		const settings = () => settingsOf(services).getState();
		const editor = screen.getByRole("link", { name: "Editor" });
		expect(editor).toHaveAttribute("href", "maxhq://settings/editor");
		expect(editor).not.toHaveAttribute("target");
		click("Editor");
		expect(settings()).toMatchObject({
			open: true,
			detail: null,
			reveal: "settings-card-open-in",
		});
		click("Codex");
		expect(settings().detail).toEqual({ kind: "login", login: "codex" });
		click("Skills");
		expect(settings().detail).toEqual({ kind: "project", project: "p1", tab: "skills" });
		click("Last");
		expect(settings().detail).toEqual({ kind: "project", project: "p1", tab: "skills" });
	});

	it("opens an agent, a note or a project's pull requests", async () => {
		const services = await answer(
			"[Agent](maxhq://agent/a1) [Note](maxhq://note/p1/n1) [Pulls](maxhq://project/p1/pulls)"
		);
		const panes = panesOf(services);
		const open = vi.spyOn(panes, "open");
		const openNote = vi.spyOn(panes, "openNote");
		click("Agent");
		expect(open).toHaveBeenCalledWith("a1", undefined);
		expect(services.ui.getState().selectedAgent).toBe("a1");
		click("Pulls");
		expect(services.ui.getState()).toMatchObject({ pullsOf: "p1", mode: "projects" });
		click("Note");
		expect(openNote).toHaveBeenCalledWith("p1", "n1", {});
		expect(services.ui.getState()).toMatchObject({ pullsOf: null, mode: "projects" });
	});

	it("opens a device's tab in Settings at a card, its page, and the devices view", async () => {
		const services = await answer(
			`[Memory](maxhq://device/${DEVBOX}/memory) [Page](maxhq://device/${DEVBOX}) ` +
				"[Devices](maxhq://view/devices) [Gone](maxhq://device/gone/memory)"
		);
		click("Memory");
		expect(settingsOf(services).getState()).toMatchObject({
			open: true,
			detail: null,
			tab: DEVBOX,
			reveal: "device-card-resources",
		});
		settingsOf(services).setState({ open: false });
		click("Page");
		expect(services.ui.getState()).toMatchObject({
			selectedLink: DEVBOX_LINK,
			mode: "devices",
		});
		services.ui.getState().setMode("projects");
		click("Gone");
		expect(services.ui.getState().mode).toBe("devices");
		services.ui.getState().setMode("projects");
		click("Devices");
		expect(services.ui.getState().mode).toBe("devices");
	});
});
