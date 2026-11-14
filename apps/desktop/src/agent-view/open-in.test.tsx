// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { panesOf } from "@/layout/panes";
import type { Agent, Entity, SshIncludeState } from "@maxhq/shared/bindings";
import type { ActionResult, CommandResult, DataSource, Editors } from "@maxhq/shared/data/source";
import { type DefaultEditor, editorReach, writeDefaultEditor } from "@maxhq/shared/editor/open";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { agent, device, entities, link, NOW, workspace } from "@maxhq/shared/test/build";
import { remoteCalls } from "@maxhq/shared/test/remote";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { AgentHeader } from "./header";
import { ItemBodyView } from "./item-row";
import { DEFAULT_EDITOR_KEY, LAST_EDITOR_KEY } from "./editor-prefs";
import { editLine, readLine } from "./open-in";

afterEach(() => {
	localStorage.clear();
	clearKept();
});

const setDefault = (choice: DefaultEditor) =>
	localStorage.setItem(DEFAULT_EDITOR_KEY, writeDefaultEditor(choice));

const DONE: ActionResult = { status: "ok", data: { status: "done" } };
const CURSOR = { id: "cursor", label: "Cursor" };
const BOTH = [CURSOR, { id: "vscode", label: "VS Code" }];
const EDITORS: Editors = { available: BOTH, known: BOTH };

const HERE = workspace({
	id: "ws-here",
	device: "dev-mac",
	location: { type: "folder", path: "/Users/someone/code/comet" },
});
// A folder on a device this Mac has no link to.
const GONE = workspace({
	id: "ws-gone",
	device: "dev-gone",
	location: { type: "folder", path: "/home/dev/comet" },
});

// A container on devbox, which an editor reaches through MaxHQ's SSH alias.
const BOX = workspace({
	id: "ws-box",
	device: "dev-devbox",
	location: { type: "container", instance: "mhq-site", path: "/home/agent/work/site" },
});
const ALIAS = "mhq-site.1a2b3c4d.maxhq";
const LINE = 'Include "/Users/someone/Library/Application Support/MaxHQ/ssh/config"';
const BLOCK = `Host ${ALIAS}\n  User agent\n  ProxyCommand ssh devbox maxd proxy --guest-ssh ${ALIAS}\n`;

const AGENT = agent({ id: "a1", workspace: HERE.id });
const IN_BOX = agent({ id: "a3", workspace: BOX.id });

const world = (): Entity[] =>
	entities({
		agents: [AGENT, agent({ id: "a2", workspace: GONE.id }), IN_BOX],
		devices: [
			device({ id: "dev-mac", name: "Mac", os: "macos" }),
			device({ id: "dev-gone", name: "devbox" }),
			device({ id: "dev-devbox", name: "site box" }),
		],
		links: [
			link({ id: "mac", name: "Mac", device: "dev-mac", route: { type: "local" } }),
			link({
				id: "devbox",
				name: "site box",
				device: "dev-devbox",
				route: { type: "ssh", host: "dev@devbox" },
			}),
		],
		workspaces: [HERE, GONE, BOX],
	});

/** Where `~/.ssh/config` stands for the container, as the backend would say. */
type Include = "ask" | "included" | "declined" | "unavailable";

const includeState = (include: Include): SshIncludeState => {
	switch (include) {
		case "ask":
			return { type: "ask", alias: ALIAS, line: LINE };
		case "included":
			return { type: "included", alias: ALIAS };
		case "declined":
			return { type: "declined", alias: ALIAS, block: BLOCK };
		case "unavailable":
			return { type: "unavailable", reason: "The container isn't running" };
	}
};

/**
 * A source over `world`, whose editor and SSH calls are spies. The SSH
 * setup starts at `include` and moves the way the backend's does.
 */
const fakeSource = (
	opened: CommandResult = { status: "ok", data: null },
	include: Include = "ask"
) => {
	let now = include;
	const source = {
		kind: "mock",
		snapshot: async () => ({ seq: 1, entities: world() }),
		subscribe: async () => () => {},
		onFocusAgent: async () => () => {},
		appInfo: async () => null,
		markSeen: async () => ({ status: "ok", data: null }),
		refreshUsage: async () => ({ status: "ok", data: null }),
		refreshLinks: async () => ({ status: "ok", data: null }),
		setShown: async () => {},
		openTimeline: async () => () => {},
		loadOlder: async () => ({ status: "ok", data: null }),
		openWorkflow: async () => () => {},
		nodeDetail: async () => ({ status: "ok", data: { lists: [] } }),
		startAgent: async () => DONE,
		sendInput: async () => DONE,
		cancelQueued: async () => DONE,
		interrupt: async () => DONE,
		setPermission: async () => DONE,
		setModel: async () => DONE,
		setEffort: async () => DONE,
		stopAgent: async () => DONE,
		setInbox: async (agents) => agents.map(() => DONE),
		continueHere: async () => DONE,
		takeOver: async () => DONE,
		answer: async () => DONE,
		stageAttachments: async () => [],
		stagePasted: async () => DONE,
		setDeviceSettings: async () => DONE,
		...remoteCalls(),
		listEditors: vi.fn<DataSource["listEditors"]>(async () => EDITORS),
		openInEditor: vi.fn<DataSource["openInEditor"]>(async () => opened),
		sshInclude: vi.fn<DataSource["sshInclude"]>(async () => includeState(now)),
		setupSshInclude: vi.fn<DataSource["setupSshInclude"]>(async (_workspace, add) => {
			if (now !== "unavailable") now = add ? "included" : "declined";
			return includeState(now);
		}),
	} satisfies DataSource;
	return source;
};

const wrap = (source: DataSource, children: ReactNode) =>
	render(
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>{children}</FixedNow>
		</ServicesProvider>
	);

const showHeader = (source: DataSource, shown: Agent = AGENT) =>
	wrap(source, <AgentHeader agent={shown} />);

/** Opens the chevron's menu and returns its only item. */
const chooseEditor = async () => {
	fireEvent.click(await screen.findByRole("button", { name: "Choose editor" }));
	const [only, ...rest] = await screen.findAllByRole("menuitem");
	if (!only || rest.length > 0) throw new Error("The menu should list one editor");
	return only;
};

describe("the header's Open in button", () => {
	it("opens the folder in the editor used last and lists the others", async () => {
		localStorage.setItem(LAST_EDITOR_KEY, "vscode");
		const source = fakeSource();
		showHeader(source);

		fireEvent.click(await screen.findByRole("button", { name: "Open in VS Code" }));
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith(
				"vscode",
				HERE.id,
				undefined,
				undefined
			)
		);

		const cursor = await chooseEditor();
		expect(cursor).toHaveTextContent("Open in Cursor");
		fireEvent.click(cursor);
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenLastCalledWith(
				"cursor",
				HERE.id,
				undefined,
				undefined
			)
		);
		// Cursor is now the one used last, here and in storage.
		expect(await screen.findByRole("button", { name: "Open in Cursor" })).toBeEnabled();
		expect(localStorage.getItem(LAST_EDITOR_KEY)).toBe("cursor");
	});

	it("starts with the first editor when the last one is gone", async () => {
		localStorage.setItem(LAST_EDITOR_KEY, "an-editor-since-removed");
		showHeader(fakeSource());
		expect(await screen.findByRole("button", { name: "Open in Cursor" })).toBeEnabled();
	});

	it("is greyed out with the reason when the folder can't open in an editor", async () => {
		const why = editorReach(GONE, undefined);
		if (why.kind !== "unavailable") throw new Error("a folder with no route should be blocked");
		const source = fakeSource();
		showHeader(source, agent({ id: "a2", workspace: GONE.id }));

		const main = await screen.findByRole("button", { name: "Open in Cursor" });
		expect(main).toBeDisabled();
		expect(main).toHaveAttribute("title", why.reason);

		const item = await chooseEditor();
		expect(item).toHaveTextContent("Open in VS Code");
		expect(item).toHaveTextContent(why.reason);
		expect(item).toHaveAttribute("aria-disabled", "true");
		expect(source.openInEditor).not.toHaveBeenCalled();
	});

	it("says below the strip when the editor doesn't open", async () => {
		showHeader(
			fakeSource({ status: "error", error: { kind: "failed", message: "Cursor quit" } })
		);
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't open Cursor. Cursor quit"
		);
	});

	it("opens the default editor, and a pick from the chevron opens once without changing it", async () => {
		setDefault({ kind: "editor", id: "vscode" });
		localStorage.setItem(LAST_EDITOR_KEY, "cursor");
		const source = fakeSource();
		showHeader(source);

		fireEvent.click(await chooseEditor());
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith(
				"cursor",
				HERE.id,
				undefined,
				undefined
			)
		);
		fireEvent.click(await screen.findByRole("button", { name: "Open in VS Code" }));
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenLastCalledWith(
				"vscode",
				HERE.id,
				undefined,
				undefined
			)
		);
		expect(localStorage.getItem(DEFAULT_EDITOR_KEY)).toBe(
			writeDefaultEditor({ kind: "editor", id: "vscode" })
		);
	});

	it("falls back to the last editor used when this Mac lacks the default", async () => {
		setDefault({ kind: "editor", id: "vscode" });
		localStorage.setItem(LAST_EDITOR_KEY, "cursor");
		const source = fakeSource();
		source.listEditors.mockResolvedValue({ available: [CURSOR], known: BOTH });
		showHeader(source);
		expect(await screen.findByRole("button", { name: "Open in Cursor" })).toBeEnabled();
	});

	it("isn't there with the default editor None", async () => {
		setDefault({ kind: "none" });
		const source = fakeSource();
		showHeader(source);
		await waitFor(() => expect(source.listEditors).toHaveBeenCalled());
		expect(screen.queryByRole("group", { name: "Open in editor" })).toBeNull();
	});

	it("isn't there when this Mac has no editor", async () => {
		const source = fakeSource();
		source.listEditors.mockResolvedValue({ available: [], known: BOTH });
		showHeader(source);
		await waitFor(() => expect(source.listEditors).toHaveBeenCalled());
		expect(screen.queryByRole("group", { name: "Open in editor" })).toBeNull();
	});
});

describe("Open in for a container", () => {
	const showBox = (include: Include) => {
		const source = fakeSource(undefined, include);
		showHeader(source, IN_BOX);
		return source;
	};

	const opensCursor = (source: ReturnType<typeof fakeSource>) =>
		waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith("cursor", BOX.id, undefined, undefined)
		);

	it("opens over SSH to the alias once your config includes MaxHQ's", async () => {
		const source = showBox("included");
		const item = await chooseEditor();
		expect(item).toHaveTextContent(`Over SSH to ${ALIAS}`);
		expect(item).not.toHaveAttribute("aria-disabled", "true");

		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		await opensCursor(source);
		expect(screen.queryByRole("dialog")).toBeNull();
		expect(source.sshInclude).toHaveBeenCalledWith(BOX.id);
	});

	it("asks first, and adds the line and opens on a yes", async () => {
		const source = showBox("ask");
		expect(await chooseEditor()).toHaveTextContent(
			`Over SSH to ${ALIAS}, after one line of setup`
		);
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		const dialog = await screen.findByRole("dialog");
		expect(dialog).toHaveTextContent("Let editors reach this container");
		expect(dialog).toHaveTextContent(
			"MaxHQ adds one line at the top of your ~/.ssh/config so editors can reach its containers"
		);
		expect(dialog).toHaveTextContent("It changes nothing else.");
		expect(screen.getByLabelText("The line MaxHQ adds")).toHaveTextContent(LINE);
		expect(source.openInEditor).not.toHaveBeenCalled();

		fireEvent.click(screen.getByRole("button", { name: "Add the line and open" }));
		await opensCursor(source);
		expect(source.setupSshInclude).toHaveBeenCalledWith(BOX.id, true);
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
	});

	it("does nothing when you cancel the question", async () => {
		const source = showBox("ask");
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
		expect(source.setupSshInclude).not.toHaveBeenCalled();
		expect(source.openInEditor).not.toHaveBeenCalled();
	});

	it("records a no and shows the entry to copy, which then opens as it is", async () => {
		const source = showBox("ask");
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		fireEvent.click(await screen.findByRole("button", { name: "I'll add it myself" }));

		expect(await screen.findByLabelText("The entry to copy")).toHaveTextContent(
			`Host ${ALIAS}`
		);
		expect(source.setupSshInclude).toHaveBeenCalledWith(BOX.id, false);
		expect(screen.getByRole("dialog")).toHaveTextContent(
			"Add this container to your SSH config"
		);
		expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled();
		expect(source.openInEditor).not.toHaveBeenCalled();

		fireEvent.click(screen.getByRole("button", { name: "Open in Cursor" }));
		await opensCursor(source);
		expect(source.setupSshInclude).toHaveBeenCalledTimes(1);
	});

	it("copies the entry after a no", async () => {
		const writeText = vi.fn(async () => {});
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		showBox("declined");
		expect(await chooseEditor()).toHaveTextContent(
			`Over SSH to ${ALIAS}, with your own SSH entry`
		);
		fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		fireEvent.click(await screen.findByRole("button", { name: "Copy" }));
		expect(writeText).toHaveBeenCalledWith(BLOCK);
		expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
	});

	it("still adds the line on a later yes, then opens", async () => {
		const source = showBox("declined");
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		fireEvent.click(await screen.findByRole("button", { name: "Let MaxHQ add the line" }));
		await opensCursor(source);
		expect(source.setupSshInclude).toHaveBeenCalledWith(BOX.id, true);
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

		// The next pick opens straight away.
		fireEvent.click(await screen.findByRole("button", { name: "Open in Cursor" }));
		await waitFor(() => expect(source.openInEditor).toHaveBeenCalledTimes(2));
		expect(screen.queryByRole("dialog")).toBeNull();
	});

	it("is greyed out with the reason when no editor reaches it", async () => {
		const source = showBox("unavailable");
		const main = await screen.findByRole("button", { name: "Open in Cursor" });
		await waitFor(() => expect(main).toHaveAttribute("title", "The container isn't running"));
		expect(main).toBeDisabled();

		const item = await chooseEditor();
		expect(item).toHaveTextContent("The container isn't running");
		expect(item).toHaveAttribute("aria-disabled", "true");
		expect(source.openInEditor).not.toHaveBeenCalled();
	});
});

describe("a file row's Open in button", () => {
	const row: RowContext = {
		agent: AGENT.id,
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote: false,
		answer: async () => DONE,
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};

	it("opens the file at the line the read started on, and leaves the row's click alone", async () => {
		const source = fakeSource();
		wrap(
			source,
			<RowProvider value={row}>
				<ItemBodyView
					item={timelineItem(
						{ kind: "file_read", path: "src/Makefile", lines: "40-80" },
						{ id: "read", status: "completed", completed_at: NOW }
					)}
				/>
			</RowProvider>
		);
		fireEvent.click(await screen.findByRole("button", { name: "Open Makefile in Cursor" }));
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith("cursor", HERE.id, "src/Makefile", 40)
		);
	});

	it("opens an edited file where the diff starts", async () => {
		const source = fakeSource();
		const patch = "--- a/src/Makefile\n+++ b/src/Makefile\n@@ -10,2 +12,3 @@\n a\n+b\n c\n";
		wrap(
			source,
			<RowProvider value={row}>
				<ItemBodyView
					item={timelineItem(
						{
							kind: "file_change",
							changes: [
								{
									path: "src/Makefile",
									old_path: null,
									op: "modify",
									additions: 1,
									deletions: 0,
									patch,
								},
							],
						},
						{ id: "edit", status: "completed", completed_at: NOW }
					)}
				/>
			</RowProvider>
		);
		const open = await screen.findByRole("button", { name: "Open Makefile in Cursor" });
		fireEvent.click(open);
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith("cursor", HERE.id, "src/Makefile", 12)
		);
		// The row itself still folds its diff open.
		expect(screen.getByRole("button", { expanded: false })).toBeInTheDocument();
	});

	it("isn't there with the default editor None", async () => {
		setDefault({ kind: "none" });
		const source = fakeSource();
		wrap(
			source,
			<RowProvider value={row}>
				<ItemBodyView
					item={timelineItem(
						{ kind: "file_read", path: "src/Makefile", lines: "40-80" },
						{ id: "read", status: "completed", completed_at: NOW }
					)}
				/>
			</RowProvider>
		);
		await waitFor(() => expect(source.listEditors).toHaveBeenCalled());
		expect(await screen.findByText("Makefile", { exact: false })).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /^Open Makefile in/ })).toBeNull();
	});
});

describe("a file row for a file the viewer shows", () => {
	const row: RowContext = {
		agent: AGENT.id,
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote: false,
		answer: async () => DONE,
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	let services: Services | null = null;
	const Grab = () => {
		services = useServices();
		return null;
	};
	const showRead = (source: DataSource, path: string) =>
		wrap(
			source,
			<RowProvider value={row}>
				<Grab />
				<ItemBodyView
					item={timelineItem(
						{ kind: "file_read", path, lines: "1-20" },
						{ id: "read", status: "completed", completed_at: NOW }
					)}
				/>
			</RowProvider>
		);

	it("opens it in a file pane", async () => {
		showRead(fakeSource(), "docs/model.glb");
		const openFile = vi.spyOn(panesOf(services!), "openFile");
		fireEvent.click(await screen.findByRole("button", { name: "Show model.glb" }));
		expect(openFile).toHaveBeenCalledWith(AGENT.id, "docs/model.glb", { line: 1 });
		expect(screen.queryByRole("button", { name: /^Open model\.glb in Cursor/ })).toBeNull();
	});

	it("opens code in a file pane at the read's first line", async () => {
		showRead(fakeSource(), "src/main.rs");
		const openFile = vi.spyOn(panesOf(services!), "openFile");
		fireEvent.click(await screen.findByRole("button", { name: "Show main.rs" }));
		expect(openFile).toHaveBeenCalledWith(AGENT.id, "src/main.rs", { line: 1 });
		expect(screen.queryByRole("button", { name: /^Open main\.rs in Cursor/ })).toBeNull();
		expect(
			screen.getByRole("button", { name: "Open main.rs in an editor" })
		).toBeInTheDocument();
	});

	it("leaves a secrets file to the editor", async () => {
		showRead(fakeSource(), "config/.env");
		expect(
			await screen.findByRole("button", { name: "Open .env in Cursor" })
		).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: "Show .env" })).toBeNull();
	});

	it("still reaches every editor from its chevron", async () => {
		const source = fakeSource();
		showRead(source, "docs/plan.md");
		fireEvent.click(await screen.findByRole("button", { name: "Open plan.md in an editor" }));
		const items = await screen.findAllByRole("menuitem");
		expect(items.map((item) => item.textContent)).toEqual([
			"Open in Cursor",
			"Open in VS Code",
		]);
		fireEvent.click(items[0]!);
		await waitFor(() =>
			expect(source.openInEditor).toHaveBeenCalledWith("cursor", HERE.id, "docs/plan.md", 1)
		);
	});

	it("shows it with the default editor None", async () => {
		setDefault({ kind: "none" });
		showRead(fakeSource(), "docs/plan.md");
		expect(await screen.findByRole("button", { name: "Show plan.md" })).toBeInTheDocument();
	});
});

describe("the line a file opens at", () => {
	it("comes from the first hunk of a diff", () => {
		expect(editLine("@@ -1 +1 @@\n-a\n+b\n")).toBe(1);
		expect(editLine("@@ -3,4 +5,6 @@ fn main\n")).toBe(5);
		expect(editLine(null)).toBeUndefined();
		expect(editLine("no hunks here")).toBeUndefined();
	});

	it("comes from the start of a read's span", () => {
		expect(readLine("10-40")).toBe(10);
		expect(readLine("7")).toBe(7);
		expect(readLine(null)).toBeUndefined();
		expect(readLine("all")).toBeUndefined();
	});
});

describe("a narrow header", () => {
	// A ResizeObserver that reports every element it watches as this wide.
	const reportWidth = (width: number) => {
		vi.stubGlobal(
			"ResizeObserver",
			class {
				constructor(private readonly report: ResizeObserverCallback) {}
				observe(target: Element) {
					const entry = { target, contentRect: { width } } as ResizeObserverEntry;
					this.report([entry], this as unknown as ResizeObserver);
				}
				unobserve() {}
				disconnect() {}
			}
		);
	};
	afterEach(() => vi.unstubAllGlobals());

	const place = () => screen.getByRole("button", { name: /^Workspace .* on Mac/ });

	it("names the device, folder and editor where there's room", async () => {
		reportWidth(1200);
		showHeader(fakeSource());
		expect(await screen.findByText("Open in Cursor")).not.toHaveClass("sr-only");
		expect(screen.getByText("comet")).not.toHaveClass("sr-only");
		expect(place()).toHaveTextContent("Mac");
	});

	it("folds them to icons and keeps their names in tooltips", async () => {
		reportWidth(400);
		showHeader(fakeSource());
		expect(await screen.findByText("Open in Cursor")).toHaveClass("sr-only");
		const folder = screen.getByText("comet");
		expect(folder).toHaveClass("sr-only");
		expect(folder.parentElement).toHaveAttribute("title", "/Users/someone/code/comet");
		expect(place()).not.toHaveTextContent("Mac");
		expect(place()).toHaveAttribute("title", expect.stringContaining("Mac"));
	});
});
