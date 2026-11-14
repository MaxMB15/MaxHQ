// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

afterEach(() => {
	vi.restoreAllMocks();
});

const openSettings = async () => {
	render(
		<FixedNow value={NOW}>
			<App
				source={mockSource({ now: () => NOW, tick: false, scenario: "connected" })}
				ui={createUiStore()}
			/>
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	const dialog = screen.getByRole("dialog", { name: "Settings" });
	const field = within(nav).getByRole("searchbox", { name: "Search settings" });
	return { nav, dialog, field };
};

const search = (field: HTMLElement, text: string) =>
	fireEvent.change(field, { target: { value: text } });

const marks = (element: HTMLElement) =>
	Array.from(element.querySelectorAll("mark"), (mark) => mark.textContent);

describe("the settings list", () => {
	it("puts the App group first and opens on its first page", async () => {
		const { nav, dialog } = await openSettings();
		const groups = within(nav)
			.getAllByRole("group")
			.map((group) => group.getAttribute("aria-label"));
		expect(groups).toEqual(["App", "Logins", "Projects"]);
		expect(
			within(dialog).getByRole("heading", { level: 2, name: "Appearance" })
		).toBeInTheDocument();
	});
});

describe("the settings pages", () => {
	it("keep their text from selecting, like the rest of the app's chrome", async () => {
		const { nav, dialog } = await openSettings();
		for (const page of within(nav).getAllByRole("button")) {
			fireEvent.click(page);
			const selectable = dialog.querySelectorAll(".select-text, .select-all");
			expect(
				Array.from(selectable, (element) => element.textContent),
				page.textContent ?? ""
			).toEqual([]);
		}
	});
});

describe("the settings search", () => {
	it("keeps the pages that have the words, and marks them", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "split");
		expect(within(nav).queryByRole("group", { name: "Logins" })).toBeNull();
		expect(within(nav).queryByRole("group", { name: "Projects" })).toBeNull();
		const app = within(nav).getByRole("group", { name: "App" });
		const [page, ...rest] = within(app).getAllByRole("button");
		expect(rest).toEqual([]);
		expect(page).toHaveTextContent(/^Keyboard shortcutsSplit down$/);
		expect(marks(page!)).toEqual(["Split"]);

		fireEvent.click(page!);
		expect(
			within(dialog).getByRole("heading", { level: 2, name: "Keyboard shortcuts" })
		).toBeInTheDocument();
		expect(within(dialog).getByText("Split right")).toBeInTheDocument();
	});

	it("matches every word, across the title and a label, and opens the first page on Enter", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "phones pairing");
		const page = within(nav).getByRole("button", { name: /^Phones/ });
		expect(page).toHaveTextContent("PhonesPairing code");
		expect(marks(page)).toEqual(["Phones", "Pairing"]);
		expect(within(nav).getAllByRole("button")).toHaveLength(1);

		fireEvent.keyDown(field, { key: "Enter" });
		expect(page).toHaveAttribute("aria-current", "page");
		expect(
			within(dialog).getByText(
				"The phones that reach your devices, and a code to pair a new one"
			)
		).toBeInTheDocument();
	});

	it("opens a project on its Skills tab when only that tab has the words", async () => {
		const { nav, dialog, field } = await openSettings();
		search(field, "proposals");
		const projects = within(nav).getByRole("group", { name: "Projects" });
		const page = within(projects).getByRole("button", { name: /^MaxHQ/ });
		expect(page).toHaveTextContent("MaxHQProposals");

		fireEvent.click(page);
		expect(within(dialog).getByRole("tab", { name: "Skills" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
	});

	it("says when nothing matches, and Esc clears the search before it closes the settings", async () => {
		const { nav, field } = await openSettings();
		search(field, "nothing has this");
		expect(within(nav).getByRole("status")).toHaveTextContent("No settings match");
		expect(within(nav).queryAllByRole("button")).toEqual([]);

		fireEvent.keyDown(field, { key: "Escape" });
		expect(field).toHaveValue("");
		expect(within(nav).getByRole("group", { name: "Logins" })).toBeInTheDocument();
		expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();

		fireEvent.keyDown(field, { key: "Escape" });
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
	});

	it("finds a device's own settings, and opens its page at the card", async () => {
		const scrolled: string[] = [];
		Element.prototype.scrollIntoView = function (this: Element) {
			scrolled.push(this.id);
		};
		try {
			const { nav, field } = await openSettings();
			search(field, "stop idle");
			const devices = within(nav).getByRole("group", { name: "Devices" });
			const [hit] = within(devices).getAllByRole("button");
			expect(hit).toHaveTextContent(/Stop idle CLIs after$/);
			expect(marks(hit!)).toEqual(["Stop", "idle"]);

			fireEvent.click(hit!);
			await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
			const card = await screen.findByRole("region", { name: /^Idle agents on / });
			await waitFor(() => expect(scrolled).toEqual([card.id]));
			expect(card.id).toBe("device-card-idle");
		} finally {
			delete (Element.prototype as Partial<Element>).scrollIntoView;
		}
	});

	it("takes Command-F to the field while the settings are open", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		const { nav, field } = await openSettings();
		const page = within(nav).getByRole("button", { name: "Editor" });
		act(() => page.focus());
		let passed = true;
		act(() => {
			passed = fireEvent.keyDown(page, { key: "f", code: "KeyF", metaKey: true });
		});
		expect(passed).toBe(false);
		expect(field).toHaveFocus();
	});
});
