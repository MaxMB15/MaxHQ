// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

import { Shell } from "@/App";
import { ServicesProvider } from "@/app/services";
import { ErrorRow } from "@/agent-view/items/notices";
import { mockSource } from "@/data/mock-source";
import type { AccountId, Entity } from "@maxhq/shared/bindings";
import {
	CLI_SIGN_IN_MS,
	MOCK_CLAUDE_PAGE,
	MOCK_CODEX_PAGE,
	MOCK_CURSOR_PAGE,
} from "@maxhq/shared/data/mock-cli-login";
import { CLAUDE, CODEX, CURSOR, DEVBOX_CLAUDE } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { CHECK_EVERY_MS } from "@maxhq/shared/settings/cli-sign-in";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { LOGIN_STEPS } from "./login-steps";

const SENT_CODE = "not-a-real-code";

let opened: MockInstance<typeof window.open>;

beforeEach(() => {
	opened = vi.spyOn(window, "open").mockImplementation(() => null);
});

afterEach(() => {
	opened.mockRestore();
});

// The sample Cursor login, as a usage check that found its CLI signed out leaves it.
const signedOut = (e: Entity): Entity =>
	e.entity === "account" && e.id === CURSOR
		? { ...e, email: null, plan: null, usage_failure: { at: NOW, reason: "logged out" } }
		: e;

/**
 * The window, with an error row that asks to sign in again to `signIn`.
 * `out` has the Cursor login signed out.
 */
const renderShell = async (signIn?: AccountId, out = false) => {
	const sample = mockSource({ now: () => NOW, tick: false });
	const source = out
		? {
				...sample,
				snapshot: async () => {
					const snapshot = await sample.snapshot();
					return { ...snapshot, entities: snapshot.entities.map(signedOut) };
				},
			}
		: sample;
	const item = signIn
		? timelineItem(
				{
					kind: "error",
					message: "Not logged in. Please run /login.",
					retrying: false,
					sign_in: signIn,
				},
				{ id: "e1" }
			)
		: null;
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<Shell />
				{item?.kind === "error" ? <ErrorRow item={item} /> : null}
			</ServicesProvider>
		</FixedNow>
	);
	// The forms tick the devices that are connected, so wait for the view.
	await screen.findAllByRole("button", { name: /devbox/ });
	return source;
};

const openCard = async (page: "claude" | "codex" | "cursor") => {
	fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(within(nav).getByRole("button", { name: LOGIN_STEPS[page].title }));
	const settings = await screen.findByRole("dialog", { name: "Settings" });
	return within(settings).getByRole("region", { name: "Sign in again" });
};

describe("Sign in again", () => {
	it("signs a Claude login on this Mac in with the code from the page", async () => {
		const source = await renderShell();
		const begin = vi.spyOn(source, "beginCliLogin");
		const finish = vi.spyOn(source, "finishCliLogin");
		const card = await openCard("claude");
		fireEvent.click(
			within(card).getByRole("button", { name: "Sign in again to Claude on Mac" })
		);

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Claude Code" });
		expect(within(dialog).getByText("Claude on Mac")).toBeInTheDocument();
		expect(
			await within(dialog).findByText(/Your browser opened to sign in/)
		).toBeInTheDocument();
		expect(begin).toHaveBeenCalledTimes(1);
		expect(begin).toHaveBeenCalledWith("dev-mac", CLAUDE, false);
		expect(within(dialog).getByRole("link", { name: "Open the sign-in page" })).toHaveAttribute(
			"href",
			MOCK_CLAUDE_PAGE
		);
		// The CLI on this Mac opens the browser itself.
		expect(opened).not.toHaveBeenCalled();

		const field = within(dialog).getByLabelText("Paste the code from the page");
		const signIn = within(dialog).getByRole("button", { name: "Sign in" });
		expect(signIn).toBeDisabled();
		fireEvent.input(field, { target: { value: ` ${SENT_CODE}\n` } });
		fireEvent.click(signIn);

		expect(
			await within(dialog).findByText(
				"Signed in as max@example.com. Send your message again to continue."
			)
		).toBeInTheDocument();
		expect(finish).toHaveBeenCalledWith("dev-mac", "cli-sign-in-1", SENT_CODE);
		expect(field).not.toBeInTheDocument();

		fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "Sign in to Claude Code" })).toBeNull()
		);
	});

	it("waits for a Codex login to finish in the browser", async () => {
		const source = await renderShell();
		const begin = vi.spyOn(source, "beginCliLogin");
		const check = vi.spyOn(source, "checkCliLogin");
		const card = await openCard("codex");
		fireEvent.click(
			within(card).getByRole("button", { name: "Sign in again to Codex on Mac" })
		);

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Codex" });
		expect(
			await within(dialog).findByText(/Your browser opened to sign in\. Finish there\./)
		).toBeInTheDocument();
		expect(within(dialog).getByText("Waiting for you to sign in")).toBeInTheDocument();
		// On the app's own Mac the CLI opens its page, without a device code.
		expect(begin).toHaveBeenCalledWith("dev-mac", CODEX, false);
		expect(within(dialog).getByRole("link", { name: "Open the sign-in page" })).toHaveAttribute(
			"href",
			MOCK_CODEX_PAGE
		);
		expect(opened).not.toHaveBeenCalled();

		expect(
			await within(dialog).findByText(
				"Signed in as max@example.net. Send your message again to continue.",
				{},
				{ timeout: CLI_SIGN_IN_MS + CHECK_EVERY_MS + 2000 }
			)
		).toBeInTheDocument();
		expect(check).toHaveBeenCalledWith("dev-mac", "cli-sign-in-1");
	});

	it("opens Cursor's page itself, on this Mac too, and waits", async () => {
		const source = await renderShell();
		const begin = vi.spyOn(source, "beginCliLogin");
		const card = await openCard("cursor");
		fireEvent.click(
			within(card).getByRole("button", { name: "Sign in again to Cursor on Mac" })
		);

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Cursor" });
		expect(
			await within(dialog).findByText(/Your browser opened to sign in\. Finish there\./)
		).toBeInTheDocument();
		expect(within(dialog).getByText("Waiting for you to sign in")).toBeInTheDocument();
		// Cursor's CLI runs without a browser and never asks for a code.
		expect(begin).toHaveBeenCalledWith("dev-mac", CURSOR, false);
		expect(opened).toHaveBeenCalledWith(MOCK_CURSOR_PAGE, "_blank", "noreferrer");
		expect(within(dialog).getByRole("link", { name: "Open the sign-in page" })).toHaveAttribute(
			"href",
			MOCK_CURSOR_PAGE
		);

		expect(
			await within(dialog).findByText(
				"Signed in as max@example.com. Send your message again to continue.",
				{},
				{ timeout: CLI_SIGN_IN_MS + CHECK_EVERY_MS + 2000 }
			)
		).toBeInTheDocument();
	});

	it("says Sign in for a login a usage check found signed out", async () => {
		await renderShell(undefined, true);
		const card = await openCard("cursor");
		expect(card).toHaveTextContent("Logged out");
		expect(
			within(card).queryByRole("button", { name: "Sign in again to Cursor on Mac" })
		).toBeNull();
		const settings = screen.getByRole("dialog", { name: "Settings" });
		const mac = within(settings).getByRole("region", { name: "On the Mac" });
		expect(mac).toHaveTextContent("Signed out");

		fireEvent.click(within(card).getByRole("button", { name: "Sign in to Cursor on Mac" }));
		const dialog = await screen.findByRole("dialog", { name: "Sign in to Cursor" });
		await within(dialog).findByText("Waiting for you to sign in");
	});

	it("stops the CLI when you close the dialog while it waits", async () => {
		const source = await renderShell();
		const begin = vi.spyOn(source, "beginCliLogin");
		const cancel = vi.spyOn(source, "cancelCliLogin");
		const card = await openCard("codex");
		fireEvent.click(
			within(card).getByRole("button", { name: "Sign in again to Codex on Mac" })
		);

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Codex" });
		await within(dialog).findByText("Waiting for you to sign in");
		fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

		await waitFor(() => expect(cancel).toHaveBeenCalledWith("dev-mac", "cli-sign-in-1"));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "Sign in to Codex" })).toBeNull()
		);
		expect(begin).toHaveBeenCalledTimes(1);
	});

	it("shows why it failed and tries again", async () => {
		const source = await renderShell();
		const begin = vi.spyOn(source, "beginCliLogin").mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "Claude isn't installed on this device." },
		});
		const card = await openCard("claude");
		fireEvent.click(
			within(card).getByRole("button", { name: "Sign in again to Claude on Mac" })
		);

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Claude Code" });
		expect(
			await within(dialog).findByText("Claude isn't installed on this device.")
		).toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("button", { name: "Try again" }));

		expect(
			await within(dialog).findByText(/Your browser opened to sign in/)
		).toBeInTheDocument();
		expect(begin).toHaveBeenCalledTimes(2);
	});

	it("opens the dialog from an error a signed-out login caused", async () => {
		const source = await renderShell(CLAUDE);
		const begin = vi.spyOn(source, "beginCliLogin");
		fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));

		const dialog = await screen.findByRole("dialog", { name: "Sign in to Claude Code" });
		await within(dialog).findByText(/Your browser opened to sign in/);
		expect(begin).toHaveBeenCalledWith("dev-mac", CLAUDE, false);
	});

	it("opens the Claude page for a login a setup-token gave, with only its device ticked", async () => {
		const source = await renderShell(DEVBOX_CLAUDE);
		const begin = vi.spyOn(source, "beginCliLogin");
		fireEvent.click(screen.getByRole("button", { name: "Sign in again" }));

		const settings = await screen.findByRole("dialog", { name: "Settings" });
		expect(
			within(settings).getByRole("heading", { name: LOGIN_STEPS.claude.title })
		).toBeInTheDocument();
		const devices = within(settings).getByRole("group", { name: "Devices" });
		expect(within(devices).getByRole("checkbox", { name: "devbox" })).toBeChecked();
		for (const box of within(devices).getAllByRole("checkbox")) {
			if (box.getAttribute("aria-label") !== "devbox") expect(box).not.toBeChecked();
		}
		expect(begin).not.toHaveBeenCalled();
	});
});
