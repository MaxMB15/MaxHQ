// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { KeyHome, KeyHomeState } from "@maxhq/shared/bindings";

import { AppKeyHomePrompt, AppKeyHomeRow } from "./app-key-home";

const FILE = "/home/<user>/.local/share/maxhq/app-key";

// The app's backend, as each test plays it: where the key is, and what a
// move answers.
const native = vi.hoisted(() => ({
	inApp: true,
	state: null as KeyHomeState | null,
	fails: null as string | null,
	appKeyHome: vi.fn(),
	keepAppKeyIn: vi.fn(),
}));

vi.mock("@/native/platform", async (original) => ({
	...(await original<typeof import("@/native/platform")>()),
	inApp: () => native.inApp,
}));

vi.mock("@/data/tauri-commands", async (original) => {
	const real = await original<typeof import("@/data/tauri-commands")>();
	return {
		...real,
		commands: {
			...real.commands,
			appKeyHome: native.appKeyHome,
			keepAppKeyIn: native.keepAppKeyIn,
		},
	};
});

beforeEach(() => {
	native.inApp = true;
	native.state = null;
	native.fails = null;
	native.appKeyHome.mockReset();
	native.appKeyHome.mockImplementation(async () => native.state);
	native.keepAppKeyIn.mockReset();
	native.keepAppKeyIn.mockImplementation(async (home: KeyHome) => {
		if (native.fails !== null)
			return { status: "error", error: { kind: "failed", message: native.fails } };
		native.state = {
			home: home === "memory" ? (native.state?.home ?? "memory") : home,
			ask: false,
			file: native.state?.file ?? null,
		};
		return { status: "ok", data: native.state };
	});
});
afterEach(cleanup);

const renderWith = (node: ReactNode) =>
	render(
		<QueryClientProvider
			client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
		>
			{node}
		</QueryClientProvider>
	);

describe("the app key question", () => {
	it("names the risk and the file, and Not now keeps the key in memory", async () => {
		native.state = { home: "memory", ask: true, file: FILE };
		renderWith(<AppKeyHomePrompt />);

		const dialog = await screen.findByRole("alertdialog", {
			name: "Where should MaxHQ keep its app key?",
		});
		expect(dialog).toHaveTextContent(/every program you run can read it too/);
		expect(dialog).toHaveTextContent(FILE);

		fireEvent.click(screen.getByRole("button", { name: "Not now" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(native.keepAppKeyIn).toHaveBeenCalledWith("memory");
	});

	it("moves the key to the file when you keep it there", async () => {
		native.state = { home: "memory", ask: true, file: FILE };
		renderWith(<AppKeyHomePrompt />);

		fireEvent.click(await screen.findByRole("button", { name: "Keep it in a file" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(native.keepAppKeyIn).toHaveBeenCalledWith("file");
	});

	it("stays open and says why when the key can't move", async () => {
		native.state = { home: "memory", ask: true, file: FILE };
		native.fails = "The data folder is read only.";
		renderWith(<AppKeyHomePrompt />);

		fireEvent.click(await screen.findByRole("button", { name: "Keep it in a file" }));
		expect(await screen.findByText("The data folder is read only.")).toBeInTheDocument();
		expect(screen.getByRole("alertdialog")).toBeInTheDocument();
	});

	it("doesn't ask once you've chosen, on a Mac or in a browser", async () => {
		native.state = { home: "keyring", ask: false, file: FILE };
		renderWith(<AppKeyHomePrompt />);
		await waitFor(() => expect(native.appKeyHome).toHaveBeenCalled());
		expect(screen.queryByRole("alertdialog")).toBeNull();

		cleanup();
		native.inApp = false;
		native.appKeyHome.mockClear();
		renderWith(<AppKeyHomePrompt />);
		expect(native.appKeyHome).not.toHaveBeenCalled();
	});
});

describe("where the key is kept, on the App keys card", () => {
	it("asks before it moves the key from the keyring to a file", async () => {
		native.state = { home: "keyring", ask: false, file: FILE };
		renderWith(<AppKeyHomeRow />);

		expect(await screen.findByText("In the system keyring.")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Move to the keyring" })).toBeNull();

		fireEvent.click(screen.getByRole("button", { name: "Keep it in a file" }));
		const dialog = await screen.findByRole("alertdialog", {
			name: "Keep the app key in a file?",
		});
		expect(dialog).toHaveTextContent(/keeps it locked while you're signed out/);
		fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
		expect(native.keepAppKeyIn).not.toHaveBeenCalled();
	});

	it("shows the file and moves the key back to the keyring", async () => {
		native.state = { home: "file", ask: false, file: FILE };
		renderWith(<AppKeyHomeRow />);

		expect(
			await screen.findByText(`In a file only your account can read: ${FILE}`)
		).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Keep it in a file" })).toBeNull();

		fireEvent.click(screen.getByRole("button", { name: "Move to the keyring" }));
		expect(await screen.findByText("In the system keyring.")).toBeInTheDocument();
		expect(native.keepAppKeyIn).toHaveBeenCalledWith("keyring");
	});
});
