// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { CloneBanner } from "@/agent-view/clone-banner";
import { ServicesProvider, useServices } from "@/app/services";
import type {
	Capabilities,
	Delta,
	Device,
	DeviceLink,
	Entity,
	Workspace,
} from "@maxhq/shared/bindings";
import type {
	ActionResult,
	CloneState,
	DataSource,
	IgnoredFile,
	IgnoredFiles,
} from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { device, entities, link, NOW, project, workspace } from "@maxhq/shared/test/build";
import { remoteCalls } from "@maxhq/shared/test/remote";
import { noteMissed } from "@maxhq/shared/workspaces/missed";

import { CopyProjectHost } from "./copy-project";
import { ProjectWorkspaces } from "./workspace-list";

const DONE: ActionResult = { status: "ok", data: { status: "done" } };

// The bindings learn `copy` and a workspace's `clone` from the protocol.
const COPIES = { transfers: true, tunnels: true, workspaces: true, copy: true } as Capabilities;

const MAC_WS = workspace({
	id: "ws-mac",
	project: "proj-comet",
	device: "dev-mac",
	location: { type: "folder", path: "/Users/someone/code/comet" },
});

const links = (over: Partial<DeviceLink> = {}): DeviceLink[] => [
	link({
		id: "mac",
		name: "Mac",
		device: "dev-mac",
		route: { type: "local" },
		capabilities: COPIES,
	}),
	link({ id: "devbox", name: "devbox", device: "dev-devbox", capabilities: COPIES, ...over }),
];

type Extra = { links?: DeviceLink[]; devices?: Device[]; workspaces?: Workspace[] };

const base = (extra: Extra = {}): Entity[] =>
	entities({
		devices: [
			device({ id: "dev-mac", name: "Mac", os: "macos" }),
			device({
				id: "dev-devbox",
				name: "devbox",
				workspace_host: { networks: ["agentbr0"], pools: ["default"] },
			}),
			...(extra.devices ?? []),
		],
		links: extra.links ?? links(),
		projects: [{ ...project("proj-comet", "Comet"), repo: "git@example.com:owner/comet.git" }],
		workspaces: [MAC_WS, ...(extra.workspaces ?? [])],
	});

const file = (path: string, size: number): IgnoredFile => ({ path, size });

/** A source over `entities`, whose copy and clone calls are spies. */
const fakeSource = (initial: Entity[], ignored: IgnoredFiles) => {
	let emit: (delta: Delta) => void = () => {};
	let seq = 1;
	const source = {
		kind: "mock",
		snapshot: async () => ({ seq: 1, entities: initial }),
		subscribe: async (onDelta) => {
			emit = onDelta;
			return () => {};
		},
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
		listIgnoredFiles: vi.fn<DataSource["listIgnoredFiles"]>(async () => ignored),
		copyProject: vi.fn<DataSource["copyProject"]>(async (_from, _target, files) => ({
			workspace: "workspace-copy",
			written: files.length,
			failed: [],
		})),
		cancelClone: vi.fn<DataSource["cancelClone"]>(async () => DONE),
		retryClone: vi.fn<DataSource["retryClone"]>(async () => DONE),
		removeClone: vi.fn<DataSource["removeClone"]>(async () => DONE),
	} satisfies DataSource;
	const upsert = (values: Entity[]) =>
		act(() => {
			seq += 1;
			emit({ seq, changes: values.map((value) => ({ op: "upsert", value })) });
		});
	return { source, upsert };
};

const showCopy = (ignored: IgnoredFiles, extra?: Extra) => {
	const fake = fakeSource(base(extra), ignored);
	render(
		<ServicesProvider source={fake.source}>
			<FixedNow value={NOW}>
				<ProjectWorkspaces project="proj-comet" />
				<CopyProjectHost />
			</FixedNow>
		</ServicesProvider>
	);
	return fake;
};

const openForm = async () => {
	const button = await screen.findByRole("button", { name: "Copy to another device…" });
	await waitFor(() => expect(button).toBeEnabled());
	fireEvent.click(button);
	return screen.findByRole("dialog");
};

describe("copy to another device", () => {
	it("ticks the small env files and copies the ones you tick into a folder clone", async () => {
		const fake = showCopy({
			files: [file(".env", 120), file(".env.big", 2_000_000), file("notes.txt", 40)],
			truncated: false,
		});
		fake.source.copyProject.mockResolvedValueOnce({
			workspace: "workspace-copy",
			written: 1,
			failed: [{ path: "notes.txt", reason: "the folder is read only" }],
		});
		const dialog = await openForm();

		expect(within(dialog).getByRole("combobox", { name: "From, Mac" })).toBeInTheDocument();
		expect(within(dialog).getByRole("combobox", { name: "To, devbox" })).toBeInTheDocument();
		expect(await within(dialog).findByRole("checkbox", { name: ".env" })).toBeChecked();
		expect(within(dialog).getByRole("checkbox", { name: ".env.big" })).not.toBeChecked();
		expect(within(dialog).getByRole("checkbox", { name: "notes.txt" })).not.toBeChecked();
		expect(fake.source.listIgnoredFiles).toHaveBeenCalledWith("ws-mac");

		fireEvent.click(within(dialog).getByRole("checkbox", { name: "notes.txt" }));
		fireEvent.click(within(dialog).getByRole("button", { name: "Folder" }));
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Parent folder" }), {
			target: { value: "/home/dev/code" },
		});
		expect(within(dialog).getByRole("button", { name: "Browse…" })).toBeInTheDocument();
		expect(within(dialog).getByRole("textbox", { name: "Folder name" })).toHaveValue("comet");
		fireEvent.click(within(dialog).getByRole("button", { name: "Copy" }));

		await waitFor(() =>
			expect(fake.source.copyProject).toHaveBeenCalledWith(
				"ws-mac",
				{ type: "folder", device: "dev-devbox", parent: "/home/dev/code", name: "comet" },
				[".env", "notes.txt"],
				expect.any(Function)
			)
		);
		expect(await within(dialog).findByText("Copied Comet to devbox")).toBeInTheDocument();
		expect(within(dialog).getByText("1 file copied.")).toBeInTheDocument();
		expect(within(dialog).getByText("1 file wasn't copied:")).toBeInTheDocument();
		expect(within(dialog).getByText("The folder is read only")).toBeInTheDocument();
	});

	it("follows the new folder's clone until the files go", async () => {
		const fake = showCopy({ files: [file(".env", 120)], truncated: false });
		let report: (sent: number) => void = () => {};
		let finish: () => void = () => {};
		fake.source.copyProject.mockImplementationOnce(
			(_from, _target, files, onProgress) =>
				new Promise((resolve) => {
					report = (sent) =>
						onProgress({
							transfer: "t1",
							sent,
							total: 120,
							file: ".env",
							state: "uploading",
						});
					finish = () =>
						resolve({ workspace: "workspace-copy", written: files.length, failed: [] });
				})
		);
		const dialog = await openForm();
		await within(dialog).findByRole("checkbox", { name: ".env" });
		fireEvent.click(within(dialog).getByRole("button", { name: "Folder" }));
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Parent folder" }), {
			target: { value: "/home/dev/code" },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Copy" }));
		expect(
			await within(dialog).findByText("Waiting for devbox to start it.")
		).toBeInTheDocument();
		expect(within(dialog).getByText("1 file to copy once it's ready.")).toBeInTheDocument();

		const clone: CloneState = {
			state: "cloning",
			stage: "receiving",
			percent: 42,
			stalled: false,
		};
		fake.upsert([
			{
				entity: "workspace",
				...workspace({
					id: "ws-new",
					project: "proj-comet",
					device: "dev-devbox",
					location: { type: "folder", path: "/home/dev/code/comet" },
				}),
				clone: { state: clone },
			} as Entity,
		]);
		const banner = await within(dialog).findByRole("region", { name: "Clone" });
		expect(banner).toHaveTextContent("Cloning: receiving objects, 42%");
		fireEvent.click(within(banner).getByRole("button", { name: "Cancel" }));
		expect(fake.source.cancelClone).toHaveBeenCalledWith("ws-new");

		// The clone is done, so the files go.
		fake.upsert([
			{
				entity: "workspace",
				...workspace({
					id: "ws-new",
					project: "proj-comet",
					device: "dev-devbox",
					location: { type: "folder", path: "/home/dev/code/comet" },
				}),
			},
		]);
		expect(
			await within(dialog).findByText("Cloned into /home/dev/code/comet.")
		).toBeInTheDocument();
		act(() => report(60));
		expect(within(dialog).getByText("Copying files: 60 B of 120 B")).toBeInTheDocument();
		act(() => finish());
		expect(await within(dialog).findByText("1 file copied.")).toBeInTheDocument();
	});

	it("warns past 100 MB and won't copy past what one transfer carries", async () => {
		showCopy({
			files: [file(".env", 120), file("data.bin", 150_000_000), file("dump.sql", 3e9)],
			truncated: true,
		});
		const dialog = await openForm();
		await within(dialog).findByRole("checkbox", { name: ".env" });
		expect(
			within(dialog).getByText("Showing the first 3 files git ignores.")
		).toBeInTheDocument();

		fireEvent.click(within(dialog).getByRole("checkbox", { name: "data.bin" }));
		expect(
			within(dialog).getByText("That's 150 MB, over 100 MB, so the copy takes a while.")
		).toBeInTheDocument();
		expect(within(dialog).getByRole("button", { name: "Copy" })).toBeEnabled();

		fireEvent.click(within(dialog).getByRole("checkbox", { name: "dump.sql" }));
		expect(
			within(dialog).getByText(
				"That's more than one copy carries, 2 GB or 20,000 files. Untick some."
			)
		).toBeInTheDocument();
		expect(within(dialog).getByRole("button", { name: "Copy" })).toBeDisabled();
	});

	it("makes a new container and greys out a device whose maxd can't take copies", async () => {
		const fake = showCopy(
			{ files: [], truncated: false },
			{
				devices: [device({ id: "dev-old", name: "old" })],
				links: [...links(), link({ id: "old", name: "old", device: "dev-old" })],
			}
		);
		const dialog = await openForm();
		expect(
			await within(dialog).findByText(
				"git ignores no files here, so only the repo is cloned."
			)
		).toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("combobox", { name: /^To,/ }));
		const options = await screen.findAllByRole("option");
		const old = options.find((o) => o.textContent?.startsWith("old"));
		expect(old).toHaveTextContent("Its maxd can't take copies yet");
		expect(old).toHaveAttribute("aria-disabled", "true");
		const devbox = options.find((o) => o.textContent?.startsWith("devbox"));
		if (!devbox) throw new Error("No devbox option");
		fireEvent.pointerDown(devbox, { pointerType: "mouse" });
		fireEvent.click(devbox);

		expect(within(dialog).getByRole("button", { name: "New container" })).toHaveAttribute(
			"aria-pressed",
			"true"
		);
		fireEvent.change(within(dialog).getByRole("textbox", { name: "Container name" }), {
			target: { value: "com-2" },
		});
		fireEvent.click(within(dialog).getByRole("button", { name: "Copy" }));
		await waitFor(() =>
			expect(fake.source.copyProject).toHaveBeenCalledWith(
				"ws-mac",
				{ type: "container", device: "dev-devbox", name: "com-2" },
				[],
				expect.any(Function)
			)
		);
		expect(
			await within(dialog).findByText("The repo is cloned. No ignored files were ticked.")
		).toBeInTheDocument();
	});
});

// Notes the missed files the way a finished copy does, once mounted.
const Missed = () => {
	const { source } = useServices();
	return (
		<button
			type="button"
			onClick={() =>
				noteMissed(source, "ws-clone", [{ path: ".env", reason: "no space left" }])
			}
		>
			Note missed
		</button>
	);
};

const showBanner = (clone: CloneState | null) => {
	const cloned = {
		...workspace({ id: "ws-clone", project: "proj-comet", device: "dev-devbox" }),
		clone: clone ? { state: clone } : null,
	} as Workspace;
	const fake = fakeSource(base({ workspaces: [cloned] }), { files: [], truncated: false });
	render(
		<ServicesProvider source={fake.source}>
			<FixedNow value={NOW}>
				<Missed />
				<CloneBanner workspace="ws-clone" />
			</FixedNow>
		</ServicesProvider>
	);
	return fake;
};

describe("clone banner", () => {
	it("says a clone stalled and cancels it", async () => {
		const fake = showBanner({
			state: "cloning",
			stage: "receiving",
			percent: 64,
			stalled: true,
		});
		const banner = await screen.findByRole("region", { name: "Clone" });
		expect(banner).toHaveTextContent("Clone stalled at 64%");
		expect(banner).toHaveTextContent("git has said nothing for a while.");
		expect(within(banner).getByRole("progressbar", { name: "Clone" })).toHaveAttribute(
			"aria-valuenow",
			"64"
		);
		fireEvent.click(within(banner).getByRole("button", { name: "Cancel" }));
		expect(fake.source.cancelClone).toHaveBeenCalledWith("ws-clone");
	});

	it("offers Retry and Remove once a clone failed, and says why a retry didn't go", async () => {
		const fake = showBanner({ state: "failed", reason: "repository not found" });
		fake.source.retryClone.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "the folder is in use" },
		});
		const banner = await screen.findByRole("region", { name: "Clone" });
		expect(banner).toHaveTextContent("Clone failed");
		expect(banner).toHaveTextContent("Repository not found.");
		expect(within(banner).queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();

		fireEvent.click(within(banner).getByRole("button", { name: "Retry" }));
		expect(fake.source.retryClone).toHaveBeenCalledWith("ws-clone");
		expect(await within(banner).findByRole("alert")).toHaveTextContent("The folder is in use.");
		fireEvent.click(within(banner).getByRole("button", { name: "Remove" }));
		expect(fake.source.removeClone).toHaveBeenCalledWith("ws-clone");
	});

	it("lists the files a copy couldn't place until you dismiss them", async () => {
		showBanner(null);
		fireEvent.click(await screen.findByRole("button", { name: "Note missed" }));
		const missed = await screen.findByRole("region", { name: "Files not copied" });
		expect(missed).toHaveTextContent("1 file wasn't copied");
		expect(missed).toHaveTextContent(".env");
		expect(missed).toHaveTextContent("No space left");
		fireEvent.click(within(missed).getByRole("button", { name: "Dismiss" }));
		expect(screen.queryByRole("region", { name: "Files not copied" })).not.toBeInTheDocument();
	});
});
