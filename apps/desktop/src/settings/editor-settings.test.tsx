// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import App from "@/App";
import { ServicesProvider } from "@/app/services";
import { DEFAULT_EDITOR_KEY } from "@/agent-view/editor-prefs";
import { ComposerNativeProvider } from "@/composer/native";
import { checkerFor, fakeNative, installBrowserFakes, underlined } from "@/composer/testing";
import { TYPING_KEY } from "@/composer/typing-prefs";
import { mockSource } from "@/data/mock-source";
import { NoteEditor } from "@/notes/editor";
import type { DataSource } from "@maxhq/shared/data/source";
import { readDefaultEditor, writeDefaultEditor } from "@maxhq/shared/editor/open";
import { readTypingPrefs } from "@maxhq/shared/editor/typing";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { EditorSettingsPage } from "./editor-settings";

afterEach(() => localStorage.clear());

// The whole app takes a while to mount and to search, which a busy machine
// stretches past the test timeout. One test opens the page from Settings, and
// the rest render the page on its own.
const openEditorSettings = async (source: DataSource = mock()) => {
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	await screen.findAllByRole("button", { name: /devbox/ });
	fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(
		within(within(nav).getByRole("group", { name: "App" })).getByRole("button", {
			name: "Editor",
		})
	);
	const dialog = await screen.findByRole("dialog", { name: "Settings" });
	return within(within(dialog).getByRole("region", { name: "Open in" }));
};

/** The page alone, with the services it reads. */
const renderEditorPage = (source: DataSource = mock()) => {
	render(
		<ServicesProvider source={source} ui={createUiStore()}>
			<EditorSettingsPage />
		</ServicesProvider>
	);
	return within(screen.getByRole("region", { name: "Open in" }));
};

const mock = () => mockSource({ now: () => NOW, tick: false, scenario: "connected" });

const saved = () => readDefaultEditor(localStorage.getItem(DEFAULT_EDITOR_KEY));

// The editors come once the app lists them, after the picker's other options.
const options = async (picker: HTMLElement) => {
	fireEvent.click(picker);
	await screen.findByRole("option", { name: "Cursor" });
	return screen.getAllByRole("option").map((o) => o.textContent);
};

const choose = async (picker: HTMLElement, label: string) => {
	fireEvent.click(picker);
	const found = (await screen.findAllByRole("option")).find((o) => o.textContent === label)!;
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

describe("the editor settings page", () => {
	it("starts on Last used and saves the editor you pick, or None", async () => {
		const page = await openEditorSettings();
		const picker = page.getByRole("combobox", { name: "Default editor, Last used" });
		expect(page.getByText(/None hides those buttons/)).toBeInTheDocument();
		expect(await options(picker)).toEqual(["Last used", "Cursor", "VS Code", "None"]);
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

		await choose(picker, "VS Code");
		expect(saved()).toEqual({ kind: "editor", id: "vscode" });
		await choose(page.getByRole("combobox", { name: "Default editor, VS Code" }), "None");
		expect(saved()).toEqual({ kind: "none" });
	});

	it("lists an editor this Mac lacks as not found", async () => {
		const source = mock();
		const page = renderEditorPage({
			...source,
			listEditors: async () => {
				const { known } = await source.listEditors();
				return { available: known.filter((e) => e.id === "cursor"), known };
			},
		});
		const picker = await page.findByRole("combobox", { name: /^Default editor/ });
		fireEvent.click(picker);
		expect(
			await screen.findByRole("option", { name: "VS Code, not found" })
		).toBeInTheDocument();
	});

	it("takes up a default another window saved", async () => {
		const page = renderEditorPage();
		const newValue = writeDefaultEditor({ kind: "none" });
		act(() => {
			window.dispatchEvent(
				new StorageEvent("storage", { key: DEFAULT_EDITOR_KEY, newValue })
			);
		});
		expect(page.getByRole("combobox", { name: "Default editor, None" })).toBeInTheDocument();
	});
});

describe("the typing settings", () => {
	beforeAll(installBrowserFakes);

	/** The Editor page and a note side by side, on a Mac with its own spell check. */
	const renderWithNote = () => {
		const { native } = fakeNative({ spelling: true });
		native.checkSpelling.mockImplementation(checkerFor(["teh"]));
		render(
			<ServicesProvider source={mock()} ui={createUiStore()}>
				<ComposerNativeProvider value={native}>
					<EditorSettingsPage />
					<NoteEditor initial="Fix teh login" onChange={() => {}} />
				</ComposerNativeProvider>
			</ServicesProvider>
		);
		return {
			page: within(screen.getByRole("region", { name: "Typing" })),
			note: screen.getByRole("textbox", { name: "Note" }),
		};
	};

	const settle = () => act(async () => void (await new Promise((done) => setTimeout(done, 0))));

	it("checks spelling with the Mac's checker by default, with no autocorrect", async () => {
		const { page, note } = renderWithNote();
		await waitFor(() => expect(underlined()).toEqual(["teh"]));
		expect(note).toHaveAttribute("spellcheck", "false");
		expect(note).toHaveAttribute("autocorrect", "off");
		expect(note).toHaveAttribute("autocapitalize", "off");
		expect(page.getByRole("switch", { name: "Check spelling" })).toBeChecked();
		const correct = page.getByRole("switch", { name: "Correct spelling automatically" });
		expect(correct).not.toBeChecked();
	});

	it("takes the underlines away when Check spelling goes off, and saves it", async () => {
		const { page, note } = renderWithNote();
		await waitFor(() => expect(underlined()).toEqual(["teh"]));
		fireEvent.click(page.getByRole("switch", { name: "Check spelling" }));
		await settle();
		expect(underlined()).toEqual([]);
		expect(note).toHaveAttribute("spellcheck", "false");
		expect(
			page.getByRole("switch", { name: "Correct spelling automatically" })
		).toHaveAttribute("aria-disabled", "true");
		expect(readTypingPrefs(localStorage.getItem(TYPING_KEY)).checkSpelling).toBe(false);
	});

	it("hands spelling to the webview, autocorrect and all, once corrections are on", async () => {
		const { page, note } = renderWithNote();
		await waitFor(() => expect(underlined()).toEqual(["teh"]));
		fireEvent.click(page.getByRole("switch", { name: "Correct spelling automatically" }));
		await settle();
		expect(underlined()).toEqual([]);
		expect(note).toHaveAttribute("spellcheck", "true");
		expect(note).toHaveAttribute("autocorrect", "on");
		expect(readTypingPrefs(localStorage.getItem(TYPING_KEY))).toEqual({
			checkSpelling: true,
			correctSpelling: true,
		});
	});
});
