// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { AUTO_CONSENT_KEY } from "@/settings/auto-consent";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

const renderApp = () => {
	const source = mockSource({ now: () => NOW, tick: false });
	const ui = createUiStore();
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={ui} />
		</FixedNow>
	);
	return { source, ui };
};

const form = () => screen.getByRole("dialog", { name: "New agent" });
const picker = (label: string) =>
	within(form()).getByRole("combobox", { name: new RegExp(`^${label},`) });

// Opens a picker and finds one of its options by its text.
const option = async (label: string, text: string) => {
	fireEvent.click(picker(label));
	const options = await screen.findAllByRole("option");
	const found = options.find((o) => o.textContent?.includes(text));
	if (!found) throw new Error(`${label} has no option "${text}"`);
	return found;
};

// Picks an option with a mouse press, which is what a Base UI item takes.
const press = (element: HTMLElement) => {
	fireEvent.pointerDown(element, { pointerType: "mouse" });
	fireEvent.click(element);
};

const tabs = () => screen.queryAllByRole("tab").map((t) => t.getAttribute("aria-label"));

describe("the new-agent keys", () => {
	afterEach(() => {
		vi.restoreAllMocks();
	});

	// The sidebar's New agent button, then the form's Start agent button.
	const buttons = async (newAgent: string) => {
		renderApp();
		const sidebar = screen.getByRole("navigation", { name: "Agents" });
		const open = await within(sidebar).findByRole("button", { name: newAgent });
		fireEvent.click(open);
		return { open, start: within(form()).getByRole("button", { name: /Start agent/ }) };
	};

	it("show with Control off a Mac", async () => {
		const { open, start } = await buttons("New agentCtrl+N");
		expect(open).toHaveAttribute("title", "New agent Ctrl+N");
		expect(open).toHaveAttribute("aria-keyshortcuts", "Control+N");
		expect(start).toHaveTextContent("Start agentCtrl+↵");
		expect(start).toHaveAttribute("title", "Start agent Ctrl+↵");
	});

	it("show with Command on a Mac", async () => {
		vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"
		);
		const { open, start } = await buttons("New agent⌘N");
		expect(open).toHaveAttribute("title", "New agent ⌘N");
		expect(open).toHaveAttribute("aria-keyshortcuts", "Meta+N");
		expect(start).toHaveTextContent("Start agent⌘↵");
		expect(start).toHaveAttribute("title", "Start agent ⌘↵");
	});
});

describe("the new-agent form", () => {
	it("starts an agent in a project and opens its pane", async () => {
		const { source } = renderApp();
		const start = vi.spyOn(source, "startAgent");
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));

		expect(picker("Project")).toHaveTextContent("MaxHQ");
		expect(picker("Runs on")).toHaveTextContent("Mac");
		expect(picker("Agent")).toHaveTextContent("Claude");
		expect(picker("Model")).toHaveTextContent("CLI default");
		expect(picker("Mode")).toHaveTextContent("Auto");
		expect(within(form()).getByText("Auto is the default on Mac.")).toBeInTheDocument();

		fireEvent.change(within(form()).getByRole("textbox", { name: "Title" }), {
			target: { value: "Write the release notes" },
		});
		fireEvent.click(within(form()).getByRole("button", { name: /Start agent/ }));
		// Auto came from the Mac's default, so the first start asks about it.
		const asked = await screen.findByRole("alertdialog");
		expect(asked).toHaveTextContent("This agent is set to Auto.");
		fireEvent.click(within(asked).getByRole("button", { name: "Use Auto" }));

		await waitFor(() => expect(tabs()).toContain("Write the release notes"));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "New agent" })).not.toBeInTheDocument()
		);
		expect(start).toHaveBeenCalledWith({
			vendor: "claude",
			account: "acct-claude-mac",
			place: { type: "workspace", workspace: "ws-mac-maxhq" },
			model: null,
			permission: "auto",
			title: "Write the release notes",
			first: null,
			branch: null,
		});
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBe("confirmed");
	});

	it("starts in Ask first, and makes it the device's default, with Ask first instead", async () => {
		const { source } = renderApp();
		const start = vi.spyOn(source, "startAgent");
		const settings = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		fireEvent.click(within(form()).getByRole("button", { name: /Start agent/ }));
		const asked = await screen.findByRole("alertdialog");
		fireEvent.click(within(asked).getByRole("button", { name: "Ask first instead" }));

		await waitFor(() =>
			expect(start).toHaveBeenCalledWith(expect.objectContaining({ permission: "ask" }))
		);
		expect(settings).toHaveBeenCalledWith(
			expect.objectContaining({ device: "dev-mac", default_permission: "ask" })
		);
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBeNull();
	});

	it("starts nothing when you close the question", async () => {
		const { source } = renderApp();
		const start = vi.spyOn(source, "startAgent");
		const settings = vi.spyOn(source, "setDeviceSettings");
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		fireEvent.click(within(form()).getByRole("button", { name: /Start agent/ }));
		await screen.findByRole("alertdialog");
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

		await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
		expect(start).not.toHaveBeenCalled();
		expect(settings).not.toHaveBeenCalled();
		expect(localStorage.getItem(AUTO_CONSENT_KEY)).toBeNull();
	});

	it("starts one in a folder on a device", async () => {
		const { source } = renderApp();
		const start = vi.spyOn(source, "startAgent");
		fireEvent.keyDown(window, { key: "n", metaKey: true });
		await screen.findByRole("dialog", { name: "New agent" });

		press(await option("Project", "Use a local folder"));
		expect(picker("Device")).toHaveTextContent("Mac");
		const folder = within(form()).getByRole("textbox", { name: "Folder" });
		expect(within(form()).getByRole("status")).toHaveTextContent(
			"Type the folder's full path, starting with /"
		);
		fireEvent.change(folder, { target: { value: "/Users/max/scratch" } });
		fireEvent.change(within(form()).getByRole("textbox", { name: "First message" }), {
			target: { value: "Look around" },
		});
		fireEvent.click(within(form()).getByRole("button", { name: /Start agent/ }));

		await waitFor(() => expect(tabs()).toContain("Look around"));
		expect(start).toHaveBeenCalledWith(
			expect.objectContaining({
				place: { type: "folder", path: "/Users/max/scratch" },
				first: { text: "Look around", attachments: [] },
			})
		);
	});

	it("suggests the device's folders as you type one, and offers Browse…", async () => {
		renderApp();
		fireEvent.keyDown(window, { key: "n", metaKey: true });
		await screen.findByRole("dialog", { name: "New agent" });
		press(await option("Project", "Use a local folder"));
		const folder = within(form()).getByRole("textbox", { name: "Folder" });
		expect(within(form()).getByRole("button", { name: "Browse…" })).toBeInTheDocument();
		act(() => folder.focus());
		fireEvent.change(folder, { target: { value: "/Users/me/Code/" } });
		const list = await within(form()).findByRole("listbox", { name: "Suggestions" });
		expect(
			within(list)
				.getAllByRole("option")
				.map((o) => o.textContent)
		).toEqual(["/Users/me/Code/Github", "/Users/me/Code/scratch"]);
		// Esc closes the suggestions and leaves the form open.
		fireEvent.keyDown(folder, { key: "Escape" });
		expect(within(form()).queryByRole("listbox")).toBeNull();
		expect(form()).toBeInTheDocument();
	});

	it("gives a container's device this Mac's git identity before the agent starts", async () => {
		const mock = mockSource({ now: () => NOW, tick: false });
		// devbox keeps nothing for MaxHQ, as before anyone opens its settings.
		const source: DataSource = {
			...mock,
			snapshot: async () => {
				const snapshot = await mock.snapshot();
				const entities = snapshot.entities.filter((e) => e.entity !== "project_settings");
				return { ...snapshot, entities };
			},
		};
		vi.spyOn(source, "gitIdentity").mockResolvedValue({
			name: "Sam Placeholder",
			email: "sam@example.com",
		});
		const save = vi.spyOn(source, "setProjectSettings");
		const start = vi.spyOn(source, "startAgent");
		render(
			<FixedNow value={NOW}>
				<App source={source} ui={createUiStore()} />
			</FixedNow>
		);
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		press(await option("Runs on", "mhq-maxhq container"));
		fireEvent.click(within(form()).getByRole("button", { name: /Start agent/ }));

		await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
		expect(start.mock.calls[0]![0].place).toEqual({
			type: "workspace",
			workspace: "ws-mhq-maxhq",
		});
		// No branch typed lets maxd name one.
		expect(start.mock.calls[0]![0].branch).toBeNull();
		expect(save).toHaveBeenCalledExactlyOnceWith({
			project: "proj-maxhq",
			device: "dev-devbox",
			resume_on_restart: true,
			ignored_ports: [],
			git_author: { name: "Sam Placeholder", email: "sam@example.com" },
		});
		expect(save.mock.invocationCallOrder[0]).toBeLessThan(start.mock.invocationCallOrder[0]!);
	});

	it("turns Plan off once Codex is picked", async () => {
		renderApp();
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));

		const claudePlan = await option("Mode", "Plan only");
		expect(claudePlan).not.toHaveAttribute("aria-disabled", "true");
		press(claudePlan);
		expect(picker("Mode")).toHaveTextContent("Plan only");

		press(await option("Agent", "Codex CLI"));
		expect(picker("Agent")).toHaveTextContent("Codex Pro");
		expect(picker("Mode")).toHaveTextContent("Auto");
		expect(await option("Mode", "Plan only")).toHaveAttribute("aria-disabled", "true");
	});

	it("shows each login's usage windows as bars", async () => {
		renderApp();
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));

		const claude = await option("Agent", "Claude Max");
		expect(claude).toHaveTextContent("5 hours, 22% used");
		expect(claude).toHaveTextContent("Week, 45% used");
		const codex = (await screen.findAllByRole("option")).find((o) =>
			o.textContent?.includes("Codex Pro")
		)!;
		expect(codex).toHaveTextContent("5 hours, at its limit");
		expect(codex).toHaveTextContent("Week, 64% used");
		const bars = [...codex.querySelectorAll("[data-window]")].map((w) => [
			w.getAttribute("data-window"),
			w.querySelector("[aria-hidden]")?.textContent,
		]);
		expect(bars).toEqual([
			["300", "5h100%"],
			["10080", "wk64%"],
		]);
	});

	it("offers Local as an agent of its own, its models by server, and their setup", async () => {
		renderApp();
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		press(await option("Agent", "Qwen3.6-35B-A3B on MacBook Pro"));
		expect(picker("Agent")).toHaveTextContent("Local");
		expect(picker("Agent")).not.toHaveTextContent("Claude");
		expect(picker("Model")).toHaveTextContent("Qwen3.6-35B-A3B");

		const qwen = await option("Model", "The default, Loaded");
		expect(qwen).toHaveTextContent("Fits");
		const list = qwen.closest<HTMLElement>("[role=listbox]")!;
		expect(within(list).getByText("This Mac")).toBeInTheDocument();
		const later = (await screen.findAllByRole("option")).find((o) =>
			o.textContent?.includes("Qwen3.8-27B")
		)!;
		expect(later).toHaveTextContent("Not downloaded");
		expect(later).toHaveTextContent("Downloading, 41%");

		press(
			(await screen.findAllByRole("option")).find(
				(o) => o.textContent === "Set up local models…"
			)!
		);
		const settings = await screen.findByRole("dialog", { name: "Settings" });
		expect(
			await within(settings).findByRole("heading", { name: "Local models", level: 2 })
		).toBeInTheDocument();
	});

	it("puts the agent on the freer device until you pick one", async () => {
		const { source } = renderApp();
		const GIB = 1024 ** 3;
		const readLoad = vi.spyOn(source, "readLoad").mockImplementation(async (devices) =>
			devices.map((device) => ({
				device,
				load: {
					sampled_at: NOW,
					cpu_util: device === "dev-devbox" ? 0.1 : 0.9,
					cpu_count: 12,
					mem_available: 12 * GIB,
					mem_total: 32 * GIB,
				},
				received_at: NOW,
			}))
		);
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));

		expect(
			await within(form()).findByText(
				"devbox: most free CPU and memory. Pick a place to choose yourself."
			)
		).toBeInTheDocument();
		expect(readLoad).toHaveBeenCalledWith(["dev-devbox", "dev-mac"]);
		expect(picker("Runs on")).toHaveTextContent("devbox");

		press(await option("Runs on", "Mac"));
		expect(picker("Runs on")).toHaveTextContent("Mac");
		expect(within(form()).queryByText(/most free CPU and memory/)).toBeNull();
	});

	it("says when it picked this Mac for its load", async () => {
		renderApp();
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		expect(
			await within(form()).findByText(
				"Mac: most free CPU and memory. Pick a place to choose yourself."
			)
		).toBeInTheDocument();
	});
});

describe("the branch field", () => {
	const SHARED = "Another agent works on this branch. They'll share its folder.";

	const openForm = async () => {
		const app = renderApp();
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		return app;
	};
	const inContainer = async () => press(await option("Runs on", "mhq-maxhq container"));
	const branch = () => within(form()).queryByRole("combobox", { name: "Branch" });
	const startButton = () => within(form()).getByRole("button", { name: /Start agent/ });
	// Typed, not pasted or autofilled, so the suggestions open.
	const type = (value: string) =>
		fireEvent.input(branch()!, { target: { value }, inputType: "insertText" });

	it("shows only for a container, and empties once the agent runs elsewhere", async () => {
		await openForm();
		press(await option("Runs on", "Mac"));
		expect(branch()).toBeNull();

		await inContainer();
		expect(branch()).toHaveAttribute("placeholder", "A new maxhq/ branch");
		type("fix/login");
		press(await option("Runs on", "Mac"));
		expect(branch()).toBeNull();
		await inContainer();
		expect(branch()).toHaveValue("");
	});

	it("sends the branch you type", async () => {
		const { source } = await openForm();
		const start = vi.spyOn(source, "startAgent");
		await inContainer();
		type(" fix/login ");
		fireEvent.click(startButton());

		await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
		expect(start.mock.calls[0]![0].branch).toBe("fix/login");
	});

	it("says what's wrong with a name and won't start until it's fixed", async () => {
		await openForm();
		await inContainer();
		type("fix login");
		expect(within(form()).getByRole("status")).toHaveTextContent("A branch can't have spaces.");
		expect(branch()).toHaveAttribute("aria-invalid", "true");
		expect(startButton()).toBeDisabled();

		type("fix-login");
		expect(within(form()).getByRole("status")).toBeEmptyDOMElement();
		expect(startButton()).toBeEnabled();
	});

	it("suggests open PRs' heads, and says when another agent works on the branch", async () => {
		await openForm();
		await inContainer();
		type("usage w");
		const suggestion = await screen.findByRole("option", { name: /usage-week/ });
		expect(suggestion).toHaveTextContent("PR #141");
		press(suggestion);
		expect(branch()).toHaveValue("usage-week");
		expect(within(form()).queryByText(SHARED)).toBeNull();
		fireEvent.click(startButton());
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "New agent" })).not.toBeInTheDocument()
		);

		// The next agent on that branch in the container shares its folder.
		fireEvent.click(await screen.findByRole("button", { name: "New agent in MaxHQ" }));
		await inContainer();
		const input = branch()!;
		type("usage-week");
		fireEvent.keyDown(input, { key: "Escape" });
		expect(within(form()).getByText(SHARED)).toBeInTheDocument();
		expect(startButton()).toBeEnabled();
	});
});
