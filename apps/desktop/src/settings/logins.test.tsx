// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";

import { Shell } from "@/App";
import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { ActionResult } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NativeFailure } from "@/native";
import { openDeviceTab, openLogin } from "@/test/settings";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { LOGIN_STEPS } from "./login-steps";
import { parseAppId, setupTokenCommand } from "./logins";
import { deviceCardId, settingsOf } from "./store";

const native = vi.hoisted(() => ({
	pickAttachments: vi.fn<() => Promise<string[]>>(),
	moveToTrash: vi.fn<(path: string) => Promise<void>>(),
}));

vi.mock("@/native", async (load) => ({
	...(await load<typeof import("@/native")>()),
	pickAttachments: native.pickAttachments,
	moveToTrash: native.moveToTrash,
}));

const SECRET = "test-not-a-real-token";
const KEY = "/Users/me/Downloads/maxhq-workspaces.2026-10-25.private-key.pem";

const renderSettings = (scenario = "connected") => {
	const source = mockSource({ now: () => NOW, tick: false, scenario });
	const ui = createUiStore();
	const services: { current: Services | null } = { current: null };
	const Grab = () => {
		services.current = useServices();
		return null;
	};
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={ui}>
				<Grab />
				<Shell />
			</ServicesProvider>
		</FixedNow>
	);
	return { source, ui, services };
};

// The accounts table's row each login opens from.
const ROW: Record<string, string> = {
	"Claude in workspaces": "Claude",
	"Codex in workspaces": "Codex",
	"Cursor in workspaces": "Cursor",
	"GitHub App": "GitHub",
};

// A login's view, or the push key's part of the Phones tab.
const openPage = async (page: string): Promise<HTMLElement> => {
	// The forms tick the devices that are connected, so wait for the view.
	await screen.findAllByRole("button", { name: /devbox/ });
	if (page === "Phone pushes") {
		const dialog = await openDeviceTab("Phones");
		return dialog.querySelector<HTMLElement>(`#${deviceCardId("apns")}`)!;
	}
	return openLogin(ROW[page]!);
};

const result = (r: ActionResult) => r;

let consoles: MockInstance[] = [];

beforeEach(() => {
	consoles = (["log", "info", "warn", "error", "debug"] as const).map((level) =>
		vi.spyOn(console, level).mockImplementation(() => undefined)
	);
	native.pickAttachments.mockResolvedValue([KEY]);
	native.moveToTrash.mockResolvedValue(undefined);
});

afterEach(() => {
	native.pickAttachments.mockReset();
	native.moveToTrash.mockReset();
});

// Everything a secret could have leaked into: the console, the stores,
// what the source holds, the browser's storage and the page.
const expectNoSecret = async (
	source: ReturnType<typeof renderSettings>["source"],
	ui: ReturnType<typeof renderSettings>["ui"],
	services: Services | null
) => {
	const logged = consoles.flatMap((spy) => spy.mock.calls.flat().map((arg) => String(arg)));
	for (const line of logged) expect(line).not.toContain(SECRET);
	const places = [
		JSON.stringify(ui.getState()),
		services ? JSON.stringify(settingsOf(services).getState()) : "",
		services ? JSON.stringify([...services.view.getState().view.tables.credential]) : "",
		JSON.stringify(await source.snapshot()),
		JSON.stringify({ ...localStorage }),
		JSON.stringify({ ...sessionStorage }),
		document.body.innerHTML,
	];
	for (const place of places) expect(place).not.toContain(SECRET);
	for (const input of document.querySelectorAll("input")) expect(input.value).not.toBe(SECRET);
};

describe("Login steps", () => {
	it("keeps planning IDs out of what you read", () => {
		const lines = Object.values(LOGIN_STEPS).flatMap((login) => [
			login.title,
			...login.before,
			...login.steps,
			...login.after,
		]);
		for (const line of lines)
			expect(line).not.toMatch(/\b[A-Z]{1,2}-\d{1,2}\b|\b[MS]\d{1,2}\b/);
	});
});

describe("Claude in workspaces", () => {
	it("shows the steps word for word and the command for the picked login", async () => {
		renderSettings();
		const dialog = await openPage("Claude in workspaces");
		const steps = within(dialog).getByRole("region", {
			name: "Steps for Claude in workspaces",
		});
		const items = within(within(steps).getByRole("list")).getAllByRole("listitem");
		expect(items.map((li) => li.textContent)).toEqual(
			LOGIN_STEPS.claude.steps.map((step, i) => `${i + 1}${step.replaceAll("`", "")}`)
		);
		expect(items).toHaveLength(4);
		expect(within(dialog).getByLabelText("Command")).toHaveTextContent("claude setup-token");
		expect(
			within(dialog).getByRole("combobox", { name: "Account, Claude" })
		).toBeInTheDocument();
		expect(setupTokenCommand("/Users/me/.claude-work")).toBe(
			"CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude setup-token"
		);
		expect(setupTokenCommand("/Users/me/Max's Claude")).toBe(
			"CLAUDE_CONFIG_DIR='/Users/me/Max'\\''s Claude' claude setup-token"
		);
	});

	it("copies the command and says it did", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			renderSettings();
			const dialog = await openPage("Claude in workspaces");
			fireEvent.click(within(dialog).getByRole("button", { name: "Copy command" }));
			expect(writeText).toHaveBeenCalledWith("claude setup-token");
			expect(
				await within(dialog).findByRole("button", { name: "Copied" })
			).toBeInTheDocument();
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("sends the token to the ticked devices and keeps it nowhere", async () => {
		const { source, ui, services } = renderSettings();
		const store = vi.spyOn(source, "storeClaudeToken");
		const dialog = await openPage("Claude in workspaces");
		const field = within(dialog).getByLabelText("Setup-token");
		expect(field).toHaveAttribute("type", "password");
		expect(within(dialog).getByRole("checkbox", { name: "devbox" })).toBeChecked();

		fireEvent.input(field, { target: { value: SECRET } });
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(store).toHaveBeenCalledWith(["dev-devbox"], "acct-claude-mac", "Claude", SECRET);
		expect(field).toHaveValue("");
		const results = await within(dialog).findByRole("list", { name: "Results" });
		expect(results).toHaveTextContent("devbox has it.");
		expect(results).toHaveTextContent("New agents there run as this login.");
		await expectNoSecret(source, ui, services.current);
	});

	it("sends the name you give the token", async () => {
		const { source } = renderSettings();
		const store = vi.spyOn(source, "storeClaudeToken");
		const dialog = await openPage("Claude in workspaces");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), {
			target: { value: "Claude for devbox" },
		});
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: { value: SECRET },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		expect(store).toHaveBeenCalledWith(
			["dev-devbox"],
			"acct-claude-mac",
			"Claude for devbox",
			SECRET
		);
	});

	it("joins a token the terminal wrapped", async () => {
		const { source, ui, services } = renderSettings();
		const store = vi.spyOn(source, "storeClaudeToken");
		const dialog = await openPage("Claude in workspaces");
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: {
				value: ` ${SECRET.slice(0, 9)}\n  ${SECRET.slice(9, 15)} ${SECRET.slice(15)}\t `,
			},
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		expect(store).toHaveBeenCalledWith(["dev-devbox"], "acct-claude-mac", "Claude", SECRET);
		await within(dialog).findByRole("list", { name: "Results" });
		await expectNoSecret(source, ui, services.current);
	});

	it("shows <secret> when a device echoes the token back", async () => {
		const { source, ui, services } = renderSettings();
		vi.spyOn(source, "storeClaudeToken").mockResolvedValue([
			result({
				status: "error",
				error: { kind: "failed", message: `claude rejected ${SECRET} as expired` },
			}),
		]);
		const dialog = await openPage("Claude in workspaces");
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: { value: SECRET },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(
			await within(dialog).findByText("devbox: Claude rejected <secret> as expired.")
		).toBeInTheDocument();
		await expectNoSecret(source, ui, services.current);
	});

	it("keeps the token out of the console when the call throws with it", async () => {
		const { source, ui, services } = renderSettings();
		vi.spyOn(source, "storeClaudeToken").mockRejectedValue(
			new Error(`the pipe broke while writing ${SECRET}`)
		);
		const dialog = await openPage("Claude in workspaces");
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: { value: SECRET },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(
			await within(dialog).findByText("Couldn't send it. Mint a new token and try again.")
		).toBeInTheDocument();
		expect(console.warn).toHaveBeenCalledWith(
			"Couldn't send the setup-token:",
			"the pipe broke while writing <secret>"
		);
		await expectNoSecret(source, ui, services.current);
	});

	it("waits for a token and a connected device", async () => {
		renderSettings("tailscale_off");
		const dialog = await openPage("Claude in workspaces");
		const box = within(dialog).getByRole("checkbox", { name: "devbox" });
		expect(box).not.toBeChecked();
		expect(box).toHaveAttribute("aria-disabled", "true");
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: { value: SECRET },
		});
		expect(within(dialog).getByRole("button", { name: "Send" })).toBeDisabled();
	});

	it("won't tick a device that has setup-tokens off", async () => {
		const { source } = renderSettings();
		const snapshot = await source.snapshot();
		const kept = snapshot.entities.find(
			(e) => e.entity === "device_settings" && e.device === "dev-devbox"
		);
		if (kept?.entity !== "device_settings") throw new Error("devbox has no settings");
		await source.setDeviceSettings({ ...kept, claude_setup_token: false });

		const dialog = await openPage("Claude in workspaces");
		const box = within(dialog).getByRole("checkbox", { name: "devbox" });
		expect(box).not.toBeChecked();
		expect(box).toHaveAttribute("aria-disabled", "true");
		expect(
			within(dialog).getByText("Setup-tokens are off. Turn them on in its settings")
		).toBeInTheDocument();
		fireEvent.input(within(dialog).getByLabelText("Setup-token"), {
			target: { value: SECRET },
		});
		expect(within(dialog).getByRole("button", { name: "Send" })).toBeDisabled();
	});

	it("forgets a token after a second click", async () => {
		const { source } = renderSettings();
		const forget = vi.spyOn(source, "forgetCredential");
		const dialog = await openPage("Claude in workspaces");
		const held = within(dialog).getByRole("region", { name: "Tokens devices hold" });
		const row = within(held).getByRole("group", { name: "On devbox" });
		expect(row).toHaveTextContent("Claude, for Claude on the Mac");
		fireEvent.click(within(row).getByRole("button", { name: "Forget" }));
		expect(forget).not.toHaveBeenCalled();
		expect(row).toHaveTextContent("Forget the token on devbox?");
		fireEvent.click(within(row).getAllByRole("button", { name: "Forget" })[0]!);
		expect(forget).toHaveBeenCalledWith("dev-devbox", "cred-nl-claude");
		expect(await within(held).findByText("No device holds a token yet.")).toBeInTheDocument();
	});
});

describe("Cursor in workspaces", () => {
	it("shows the Mac's own login with its plan and a usage link", async () => {
		renderSettings();
		const dialog = await openPage("Cursor in workspaces");
		const mac = within(dialog).getByRole("region", { name: "On the Mac" });
		expect(mac).toHaveTextContent("Cursor Pro");
		expect(mac).toHaveTextContent("Signed in as max@example.com");
		expect(within(mac).getByRole("link", { name: "Manage usage" })).toHaveAttribute(
			"href",
			"https://cursor.com/dashboard"
		);
	});

	it("sends the key to the ticked devices and keeps it nowhere", async () => {
		const { source, ui, services } = renderSettings();
		const store = vi.spyOn(source, "storeCursorKey");
		const dialog = await openPage("Cursor in workspaces");
		const field = within(dialog).getByLabelText("API key");
		expect(field).toHaveAttribute("type", "password");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), {
			target: { value: " Personal " },
		});
		fireEvent.input(field, { target: { value: ` ${SECRET.slice(0, 9)}\n${SECRET.slice(9)}` } });
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(store).toHaveBeenCalledWith(["dev-devbox"], "Personal", SECRET);
		expect(field).toHaveValue("");
		const results = await within(dialog).findByRole("list", { name: "Results" });
		expect(results).toHaveTextContent("devbox has it.");
		const held = within(dialog).getByRole("region", { name: "Keys devices hold" });
		expect(await within(held).findByText(/Personal/)).toBeInTheDocument();
		await expectNoSecret(source, ui, services.current);
	});

	it("shows <secret> when a device echoes the key back", async () => {
		const { source, ui, services } = renderSettings();
		vi.spyOn(source, "storeCursorKey").mockResolvedValue([
			result({
				status: "error",
				error: { kind: "failed", message: `cursor rejected ${SECRET}` },
			}),
		]);
		const dialog = await openPage("Cursor in workspaces");
		fireEvent.input(within(dialog).getByLabelText("API key"), { target: { value: SECRET } });
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		expect(
			await within(dialog).findByText("devbox: Cursor rejected <secret>.")
		).toBeInTheDocument();
		await expectNoSecret(source, ui, services.current);
	});
});

describe("Codex in workspaces", () => {
	it("signs a device in to Codex and waits while you do", async () => {
		const { source } = renderSettings();
		let finish: (r: ActionResult) => void = () => undefined;
		const signIn = vi.spyOn(source, "signInChatGpt").mockImplementation(
			() =>
				new Promise((done) => {
					finish = done;
				})
		);
		const dialog = await openPage("Codex in workspaces");
		const steps = within(dialog).getByRole("region", { name: "Steps for Codex in workspaces" });
		expect(within(steps).getAllByRole("listitem")[0]).toHaveTextContent(
			"In Settings, Accounts, open Codex and click Continue with ChatGPT next to a device."
		);
		expect(within(dialog).getByText("Signed in as max@example.net")).toBeInTheDocument();

		const button = within(dialog).getByRole("button", {
			name: "Continue with ChatGPT on devbox",
		});
		fireEvent.click(button);
		expect(signIn).toHaveBeenCalledWith("dev-devbox");
		expect(
			await within(dialog).findByText("Waiting for you to sign in in the browser")
		).toBeInTheDocument();
		expect(button).toBeDisabled();

		finish({ status: "ok", data: { status: "done" } });
		expect(await within(dialog).findByText("Signed in")).toBeInTheDocument();
		expect(button).toBeEnabled();
	});

	it("says why the sign-in didn't finish", async () => {
		const { source } = renderSettings();
		vi.spyOn(source, "signInChatGpt").mockResolvedValue({
			status: "ok",
			data: { status: "refused", reason: "the sign-in timed out after 15 minutes" },
		});
		const dialog = await openPage("Codex in workspaces");
		fireEvent.click(
			within(dialog).getByRole("button", { name: "Continue with ChatGPT on devbox" })
		);
		expect(
			await within(dialog).findByText("The sign-in timed out after 15 minutes.")
		).toBeInTheDocument();
	});

	it("signs a device out and links to the plan's usage", async () => {
		const { source } = renderSettings();
		const forget = vi.spyOn(source, "forgetCredential");
		const dialog = await openPage("Codex in workspaces");
		const held = within(dialog).getByRole("region", { name: "Sessions devices hold" });
		const row = within(held).getByRole("group", { name: "On devbox" });
		expect(
			within(row).getByRole("link", { name: "Manage usage in ChatGPT settings" })
		).toHaveAttribute("href", "https://chatgpt.com/#settings/Usage");
		fireEvent.click(within(row).getByRole("button", { name: "Sign out" }));
		expect(row).toHaveTextContent("Sign out the Codex session on devbox?");
		fireEvent.click(within(row).getAllByRole("button", { name: "Sign out" })[0]!);
		expect(forget).toHaveBeenCalledWith("dev-devbox", expect.any(String));
		expect(await within(held).findByText("No device has signed in yet.")).toBeInTheDocument();
	});

	it("says when OpenAI didn't confirm the sign-out", async () => {
		const { source } = renderSettings();
		vi.spyOn(source, "forgetCredential").mockResolvedValue({
			status: "ok",
			data: { status: "signed_out", revoked: false },
		});
		const dialog = await openPage("Codex in workspaces");
		const row = within(dialog).getByRole("group", { name: "On devbox" });
		fireEvent.click(within(row).getByRole("button", { name: "Sign out" }));
		fireEvent.click(within(row).getAllByRole("button", { name: "Sign out" })[0]!);
		expect(
			await within(row).findByText(/OpenAI didn't confirm it ended the session/)
		).toBeInTheDocument();
	});
});

describe("GitHub App", () => {
	const fill = async (dialog: HTMLElement, appId = "1234567") => {
		fireEvent.change(within(dialog).getByRole("textbox", { name: "App ID" }), {
			target: { value: appId },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose file" }));
		await within(dialog).findByText(KEY);
	};

	it("sends the App ID and the picked key's path, then offers the Trash", async () => {
		const { source } = renderSettings();
		const send = vi.spyOn(source, "setGithubApp");
		const dialog = await openPage("GitHub App");
		expect(
			within(dialog)
				.getAllByRole("listitem")
				.some((li) =>
					li.textContent?.includes(
						"MaxHQ checks the setup. Each device mints a token for one picked repo and lists the repos the App reaches."
					)
				)
		).toBe(true);
		expect(within(dialog).queryByRole("textbox", { name: /key/i })).toBeNull();
		await fill(dialog);
		expect(native.pickAttachments).toHaveBeenCalled();
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(send).toHaveBeenCalledWith(["dev-devbox", "dev-mac"], 1234567, KEY);
		const results = await within(dialog).findByRole("list", { name: "Results" });
		expect(results).toHaveTextContent("devbox has it.");
		expect(results).toHaveTextContent("Mac has it.");
		expect(results).toHaveTextContent(
			"Reaches no repo yet. Install the App on the repos your projects use."
		);

		const offer = within(dialog).getByRole("group", { name: "Key file" });
		expect(offer).toHaveTextContent(
			"Every device has the key, so the Mac doesn't need maxhq-workspaces.2026-10-25.private-key.pem."
		);
		fireEvent.click(within(offer).getByRole("button", { name: "Move to Trash" }));
		expect(native.moveToTrash).toHaveBeenCalledWith(KEY);
		expect(
			await within(dialog).findByText(
				"Moved maxhq-workspaces.2026-10-25.private-key.pem to the Trash. The Mac keeps no copy."
			)
		).toBeInTheDocument();
	});

	it("goes to any device, while a Claude login goes only where workspaces run", async () => {
		renderSettings();
		const app = within(await openPage("GitHub App")).getByRole("group", { name: "Devices" });
		expect(within(app).getByRole("checkbox", { name: "devbox" })).toBeChecked();
		expect(within(app).getByRole("checkbox", { name: "Mac" })).toBeChecked();

		fireEvent.click(screen.getByRole("button", { name: "Back" }));
		const claude = within(await openLogin("Claude")).getByRole("group", { name: "Devices" });
		expect(within(claude).getByRole("checkbox", { name: "devbox" })).toBeInTheDocument();
		expect(within(claude).queryByRole("checkbox", { name: "Mac" })).toBeNull();
	});

	it("lists the repos each device's App reaches", async () => {
		renderSettings();
		const dialog = await openPage("GitHub App");
		const held = within(dialog).getByRole("region", { name: "Apps devices hold" });
		const row = within(held).getByRole("group", { name: "On devbox" });
		expect(row).toHaveTextContent("App 1000001, maxhq-workspaces.");
		expect(row).toHaveTextContent("Reaches Example-User/MaxHQ.");
	});

	it("checks a device's App again, and says why when it can't", async () => {
		const { source } = renderSettings();
		let answer: (r: ActionResult) => void = () => undefined;
		const check = vi
			.spyOn(source, "checkGithubApp")
			.mockImplementation(() => new Promise((resolve) => (answer = resolve)));
		const dialog = await openPage("GitHub App");
		const held = within(dialog).getByRole("region", { name: "Apps devices hold" });
		const row = within(held).getByRole("group", { name: "On devbox" });

		fireEvent.click(within(row).getByRole("button", { name: "Check again" }));
		expect(check).toHaveBeenCalledWith("dev-devbox");
		expect(within(row).getByRole("button", { name: "Checking" })).toBeDisabled();
		answer(result({ status: "ok", data: { status: "done" } }));
		expect(await within(row).findByRole("button", { name: "Check again" })).toBeEnabled();
		expect(within(row).queryByRole("status")).toBeNull();

		const tooOld =
			"devbox's maxd is too old to check when asked. Update it, or wait for it to check again by itself";
		check.mockResolvedValue(
			result({ status: "error", error: { kind: "failed", message: tooOld } })
		);
		fireEvent.click(within(row).getByRole("button", { name: "Check again" }));
		expect(await within(row).findByRole("status")).toHaveTextContent(`${tooOld}.`);
		expect(within(row).getByRole("button", { name: "Check again" })).toBeEnabled();

		check.mockRejectedValue(new Error("the link dropped"));
		fireEvent.click(within(row).getByRole("button", { name: "Check again" }));
		expect(
			await within(row).findByText("Couldn't reach the device. Try again.")
		).toBeInTheDocument();
	});

	it("doesn't offer the Trash until every device has the key", async () => {
		const { source } = renderSettings();
		vi.spyOn(source, "setGithubApp").mockResolvedValue([
			result({
				status: "error",
				error: { kind: "failed", message: "GitHub refused the key for App 1234567" },
			}),
			result({ status: "ok", data: { status: "done" } }),
		]);
		const dialog = await openPage("GitHub App");
		await fill(dialog);
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		expect(
			await within(dialog).findByText("devbox: GitHub refused the key for App 1234567.")
		).toBeInTheDocument();
		expect(within(dialog).queryByRole("group", { name: "Key file" })).toBeNull();
		expect(native.moveToTrash).not.toHaveBeenCalled();
	});

	it("says why the key couldn't go to the Trash", async () => {
		renderSettings();
		native.moveToTrash.mockRejectedValue(new NativeFailure({ kind: "not_offered", path: KEY }));
		const dialog = await openPage("GitHub App");
		await fill(dialog);
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		fireEvent.click(await within(dialog).findByRole("button", { name: "Move to Trash" }));
		expect(
			await within(dialog).findByText(
				`Couldn't move maxhq-workspaces.2026-10-25.private-key.pem to the Trash: ${KEY} wasn't dropped, pasted or picked. Delete it in Finder.`
			)
		).toBeInTheDocument();
	});

	it("needs a numeric App ID and a picked key", async () => {
		const { source } = renderSettings();
		const send = vi.spyOn(source, "setGithubApp");
		native.pickAttachments.mockResolvedValue([]);
		const dialog = await openPage("GitHub App");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "App ID" }), {
			target: { value: "maxhq-workspaces" },
		});
		expect(
			within(dialog).getByText("The App ID is a number, such as 1234567.")
		).toBeInTheDocument();
		fireEvent.change(within(dialog).getByRole("textbox", { name: "App ID" }), {
			target: { value: "1234567" },
		});
		expect(parseAppId(" 1234 567\n")).toBe(1234567);
		expect(parseAppId("0")).toBeNull();
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose file" }));
		await waitFor(() => expect(native.pickAttachments).toHaveBeenCalled());
		expect(within(dialog).getByRole("button", { name: "Send" })).toBeDisabled();
		fireEvent.submit(within(dialog).getByRole("button", { name: "Send" }));
		expect(send).not.toHaveBeenCalled();
	});
});

describe("Phone pushes", () => {
	const P8 = "/Users/me/Downloads/AuthKey_ABC123DEFG.p8";

	it("sends the trimmed IDs and the picked key's path to devices that push", async () => {
		const { source } = renderSettings();
		const send = vi.spyOn(source, "setApnsKey");
		native.pickAttachments.mockResolvedValue([P8]);
		const dialog = await openPage("Phone pushes");

		const devices = within(dialog).getByRole("group", { name: "Devices" });
		expect(within(devices).getByRole("checkbox", { name: "Mac" })).toBeChecked();
		expect(within(devices).queryByRole("checkbox", { name: "devbox" })).toBeNull();

		fireEvent.change(within(dialog).getByRole("textbox", { name: "Key ID" }), {
			target: { value: " ABC123DEFG " },
		});
		expect(within(dialog).getByRole("button", { name: "Send" })).toBeDisabled();
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Team ID" }), {
			target: { value: "<TEAM_ID>\n" },
		});
		expect(within(dialog).getByRole("button", { name: "Send" })).toBeDisabled();
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose file" }));
		await within(dialog).findByText(P8);
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));

		expect(send).toHaveBeenCalledWith(["dev-mac"], "ABC123DEFG", "<TEAM_ID>", P8);
		const results = await within(dialog).findByRole("list", { name: "Results" });
		expect(results).toHaveTextContent("Mac has it.");
		const held = within(dialog).getByRole("region", { name: "Push keys devices hold" });
		expect(within(held).getByRole("group", { name: "On Mac" })).toHaveTextContent(
			"Key ABC123DEFG, team <TEAM_ID>."
		);

		const offer = within(dialog).getByRole("group", { name: "Key file" });
		fireEvent.click(within(offer).getByRole("button", { name: "Move to Trash" }));
		expect(native.moveToTrash).toHaveBeenCalledWith(P8);
		expect(
			await within(dialog).findByText(
				"Moved AuthKey_ABC123DEFG.p8 to the Trash. The Mac keeps no copy."
			)
		).toBeInTheDocument();
	});

	it("says why a device couldn't take the key, and keeps the file", async () => {
		const { source } = renderSettings();
		vi.spyOn(source, "setApnsKey").mockResolvedValue([
			result({
				status: "error",
				error: {
					kind: "failed",
					message: "Mac's maxd is too old to push to phones. Update it first",
				},
			}),
		]);
		native.pickAttachments.mockResolvedValue([P8]);
		const dialog = await openPage("Phone pushes");
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Key ID" }), {
			target: { value: "ABC123DEFG" },
		});
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Team ID" }), {
			target: { value: "<TEAM_ID>" },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Choose file" }));
		await within(dialog).findByText(P8);
		fireEvent.click(within(dialog).getByRole("button", { name: "Send" }));
		expect(
			await within(dialog).findByText(
				"Mac: Mac's maxd is too old to push to phones. Update it first."
			)
		).toBeInTheDocument();
		expect(within(dialog).queryByRole("group", { name: "Key file" })).toBeNull();
	});
});
