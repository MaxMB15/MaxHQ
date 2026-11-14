// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

import { Shell } from "@/App";
import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { SIGN_IN_CHECK_MS } from "@/pulls/queries";
import { MOCK_GITHUB_LOGIN, SIGN_IN_MS } from "@maxhq/shared/data/mock-pulls";
import type { ActionResult } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { appSettingsUrl } from "./github-sign-in";

const CODE_PAGE = "https://github.com/login/device";
const LATER = NOW + 16 * 60_000;

let opened: MockInstance<typeof window.open>;

beforeEach(() => {
	opened = vi.spyOn(window, "open").mockImplementation(() => null);
});

afterEach(() => {
	opened.mockRestore();
});

let ui = createUiStore();

const tree = (source: ReturnType<typeof mockSource>, now: number): ReactNode => (
	<FixedNow value={now}>
		<ServicesProvider source={source} ui={ui}>
			<Shell />
		</ServicesProvider>
	</FixedNow>
);

const renderGithub = async (change?: (source: ReturnType<typeof mockSource>) => void) => {
	const source = mockSource({ now: () => NOW, tick: false });
	change?.(source);
	ui = createUiStore();
	const view = render(tree(source, NOW));
	await screen.findAllByRole("button", { name: /devbox/ });
	fireEvent.click(screen.getByRole("button", { name: "Settings" }));
	const nav = await screen.findByRole("navigation", { name: "Settings pages" });
	fireEvent.click(within(nav).getByRole("button", { name: "GitHub App" }));
	const dialog = await screen.findByRole("dialog", { name: "Settings" });
	const card = within(dialog).getByRole("region", { name: "Merge and approve as you" });
	return { source, card, rerender: (now: number) => view.rerender(tree(source, now)) };
};

describe("Merge and approve as you", () => {
	it("signs in on the device that holds the App, then signs out after asking", async () => {
		const { source, card } = await renderGithub();
		const begin = vi.spyOn(source, "beginGithubSignIn");
		expect(within(card).getByRole("combobox", { name: "Device, devbox" })).toBeInTheDocument();
		fireEvent.click(within(card).getByRole("button", { name: "Sign in to GitHub" }));

		expect(await within(card).findByText("WDJB-MJHT")).toBeInTheDocument();
		expect(begin).toHaveBeenCalledWith("dev-devbox", null);
		expect(within(card).getByText(/Waiting for GitHub/)).toBeInTheDocument();
		expect(within(card).getByRole("link", { name: "open it here" })).toHaveAttribute(
			"href",
			CODE_PAGE
		);
		expect(opened).toHaveBeenCalledTimes(1);
		expect(opened).toHaveBeenCalledWith(CODE_PAGE, "_blank", "noreferrer");

		expect(
			await within(card).findByText(
				`Signed in as @${MOCK_GITHUB_LOGIN} on devbox`,
				{},
				{ timeout: SIGN_IN_MS + 2000 }
			)
		).toBeInTheDocument();
		expect(opened).toHaveBeenCalledTimes(1);

		fireEvent.click(within(card).getByRole("button", { name: "Sign out" }));
		expect(within(card).getByText("Sign out of GitHub on devbox?")).toBeInTheDocument();
		const buttons = within(card).getAllByRole("button", { name: "Sign out" });
		fireEvent.click(buttons[buttons.length - 1]!);
		expect(
			await within(card).findByRole("button", { name: "Sign in to GitHub" })
		).toBeInTheDocument();
		expect(within(card).queryByText("WDJB-MJHT")).toBeNull();
	}, 10_000);

	it("stops waiting on Cancel and has the device forget the code", async () => {
		const { source, card } = await renderGithub();
		const cancel = vi.spyOn(source, "cancelGithubSignIn");
		fireEvent.click(within(card).getByRole("button", { name: "Sign in to GitHub" }));
		await within(card).findByText("WDJB-MJHT");
		fireEvent.click(within(card).getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(cancel).toHaveBeenCalledWith("dev-devbox", "sign-in-1"));
		expect(
			await within(card).findByRole("button", { name: "Sign in to GitHub" })
		).toBeInTheDocument();
	});

	it("says what to tick in the App when the code runs out, and tries again", async () => {
		const { source, card, rerender } = await renderGithub();
		const begin = vi.spyOn(source, "beginGithubSignIn");
		fireEvent.click(within(card).getByRole("button", { name: "Sign in to GitHub" }));
		await within(card).findByText("WDJB-MJHT");
		rerender(LATER);

		expect(await within(card).findByText("The code ran out.")).toBeInTheDocument();
		expect(within(card).getByText(/Enable Device Flow/)).toHaveTextContent(
			'Tick it in the App\'s settings under General, "Identifying and authorizing users".'
		);
		expect(within(card).getByRole("link", { name: "Open the App's settings" })).toHaveAttribute(
			"href",
			appSettingsUrl("maxhq-workspaces")
		);
		fireEvent.click(within(card).getByRole("button", { name: "Try again" }));
		await waitFor(() => expect(begin).toHaveBeenCalledTimes(2));
		expect(begin).toHaveBeenLastCalledWith("dev-devbox", null);
	});

	it("says why when GitHub turns the code down", async () => {
		const denied: ActionResult = {
			status: "ok",
			data: { status: "refused", reason: "The sign-in was cancelled on GitHub." },
		};
		const { source, card } = await renderGithub();
		const check = vi.spyOn(source, "checkGithubSignIn").mockResolvedValue(denied);
		fireEvent.click(within(card).getByRole("button", { name: "Sign in to GitHub" }));
		await within(card).findByText("WDJB-MJHT");
		expect(
			await within(card).findByText(
				"devbox: The sign-in was cancelled on GitHub.",
				{},
				{ timeout: SIGN_IN_CHECK_MS + 500 }
			)
		).toBeInTheDocument();
		expect(check).toHaveBeenCalledWith("dev-devbox", "sign-in-1");
		expect(within(card).queryByText("WDJB-MJHT")).toBeNull();
		expect(within(card).getByRole("button", { name: "Sign in to GitHub" })).toBeEnabled();
		// The check waits out its real interval, which leaves little of the
		// default timeout for the app to mount on a busy machine.
	}, 10_000);

	it("shows the device's refusal as it comes", async () => {
		const refused: ActionResult = {
			status: "ok",
			data: { status: "refused", reason: "The App on this device has no device flow" },
		};
		const { card } = await renderGithub((source) => {
			vi.spyOn(source, "beginGithubSignIn").mockResolvedValue(refused);
		});
		fireEvent.click(within(card).getByRole("button", { name: "Sign in to GitHub" }));
		expect(
			await within(card).findByText("devbox: The App on this device has no device flow.")
		).toBeInTheDocument();
		expect(within(card).getByRole("button", { name: "Sign in to GitHub" })).toBeEnabled();
	});
});

describe("appSettingsUrl", () => {
	it("is the App's settings page on GitHub", () => {
		expect(appSettingsUrl("my-app")).toBe("https://github.com/settings/apps/my-app");
	});
});
