// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { type DeviceId, type Entity, FOLDERS_SINCE } from "@maxhq/shared/bindings";
import { DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import type { DataSource } from "@maxhq/shared/data/source";
import type { PathWants } from "@maxhq/shared/folders/path";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { PathField } from "./path-field";

// The app's backend and the window's drops, as each test plays them.
const native = vi.hoisted(() => ({
	inApp: false,
	picked: null as string | null,
	pickPath: vi.fn(),
	describePaths: vi.fn(),
	over: false,
	drop: null as ((paths: string[]) => void) | null,
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
			pickPath: native.pickPath,
			describePaths: native.describePaths,
		},
	};
});

vi.mock("@/native/drop", () => ({
	useDropTarget: (onDrop: (paths: string[]) => void) => {
		native.drop = onDrop;
		return { ref: () => {}, over: native.over };
	},
}));

beforeEach(() => {
	native.inApp = false;
	native.picked = null;
	native.over = false;
	native.drop = null;
	native.pickPath.mockReset();
	native.pickPath.mockImplementation(async () => ({ status: "ok", data: native.picked }));
	native.describePaths.mockReset();
});
afterEach(cleanup);

/** The sample source, its snapshot changed by `change`. */
const sourceWith = (change: (entities: Entity[]) => Entity[] = (e) => e): DataSource => {
	const source = mockSource({ now: () => NOW, tick: false });
	const snapshot = source.snapshot;
	Object.assign(source, {
		snapshot: async () => {
			const state = await snapshot();
			return { ...state, entities: change(state.entities) };
		},
	});
	return source;
};

/** devbox's maxd from before folders. */
const olderDevbox = (entities: Entity[]): Entity[] =>
	entities.map((e) =>
		e.entity === "device_link" && e.device === DEVBOX
			? { ...e, protocol: FOLDERS_SINCE - 1 }
			: e
	);

/** devbox with a checkout of Comet in a folder. */
const devboxComet = (entities: Entity[]): Entity[] => [
	...entities,
	{
		entity: "workspace",
		id: "ws-devbox-comet",
		project: "proj-comet",
		device: DEVBOX,
		location: { type: "folder", path: "/home/dev/code/Comet" },
	},
];

type Shown = { device: DeviceId; wants?: PathWants; start?: string; source?: DataSource };

/** A path field in a form, its changes spied on, and keys that leave it. */
const show = (shown: Shown) => {
	const changes = vi.fn<(value: string) => void>();
	const outer = vi.fn();
	const Form = () => {
		const [value, setValue] = useState(shown.start ?? "");
		return (
			<div onKeyDown={outer}>
				<PathField
					device={shown.device}
					value={value}
					onChange={(next) => {
						changes(next);
						setValue(next);
					}}
					label="Folder"
					title="Choose the agent's folder"
					wants={shown.wants}
					className="h-8"
				/>
			</div>
		);
	};
	render(
		<ServicesProvider source={shown.source ?? sourceWith()} ui={createUiStore()}>
			<Form />
		</ServicesProvider>
	);
	const box = screen.getByRole("textbox", { name: "Folder" });
	return { box, changes, outer };
};

const browse = (name = "Browse…") => screen.getByRole("button", { name });

/** Waits for the view, which tells the field the device's maxd. */
const ready = () => waitFor(() => expect(browse()).not.toHaveAttribute("aria-disabled"));

const optionsOf = (list: HTMLElement) =>
	within(list)
		.getAllByRole("option")
		.map((o) => o.textContent);

const type = (box: HTMLElement, text: string) => {
	act(() => box.focus());
	fireEvent.change(box, { target: { value: text } });
};

describe("the path field's suggestions", () => {
	it("lists the folder you type in on the field's device, checkouts tagged", async () => {
		const { box, changes } = show({ device: DEVBOX });
		await ready();
		type(box, "~/code/");
		const list = await screen.findByRole("listbox", { name: "Suggestions" });
		expect(optionsOf(list)).toEqual([
			"~/code/DuskToolsgit · dev",
			"~/code/Mosaicgit · main",
			"~/code/notes",
			"~/code/scratch",
		]);
		expect(list).toHaveTextContent("Tab completes · ↩ picks · on devbox");
		// The form gets the path with ~ read once the device says where home is.
		await waitFor(() => expect(changes).toHaveBeenLastCalledWith("/home/dev/code/"));
	});

	it("moves with the arrows, completes with Tab, picks with ↩ and closes with Esc", async () => {
		const { box, changes, outer } = show({ device: DEVBOX });
		await ready();
		type(box, "~/code/");
		const list = await screen.findByRole("listbox", { name: "Suggestions" });
		// With no name typed, nothing is marked until you move.
		expect(box).not.toHaveAttribute("aria-activedescendant");
		fireEvent.keyDown(box, { key: "ArrowDown" });
		fireEvent.keyDown(box, { key: "ArrowDown" });
		const second = within(list).getAllByRole("option")[1];
		expect(second).toHaveAttribute("aria-selected", "true");
		expect(box).toHaveAttribute("aria-activedescendant", second?.id);

		fireEvent.keyDown(box, { key: "Tab" });
		expect(box).toHaveValue("~/code/Mosaic/");
		await waitFor(() => expect(changes).toHaveBeenLastCalledWith("/home/dev/code/Mosaic/"));

		type(box, "~/code/sc");
		expect(optionsOf(await screen.findByRole("listbox"))).toEqual(["~/code/scratch"]);
		fireEvent.keyDown(box, { key: "Enter" });
		expect(box).toHaveValue("~/code/scratch");
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(changes).toHaveBeenLastCalledWith("/home/dev/code/scratch");

		type(box, "~/code/");
		await screen.findByRole("listbox");
		fireEvent.keyDown(box, { key: "Escape" });
		expect(screen.queryByRole("listbox")).toBeNull();
		// Esc closes the list, not the dialog around the field.
		expect(outer).not.toHaveBeenCalledWith(expect.objectContaining({ key: "Escape" }));
	});

	it("keeps what you typed on ↩ when no suggestion starts with it, until you move to one", async () => {
		const { box, changes, outer } = show({ device: DEVBOX });
		await ready();
		type(box, "~/code/osa");
		const list = await screen.findByRole("listbox");
		expect(optionsOf(list)).toEqual(["~/code/Mosaicgit · main"]);
		expect(box).not.toHaveAttribute("aria-activedescendant");
		// The ↩ isn't taken, so the form around the field gets it.
		expect(fireEvent.keyDown(box, { key: "Enter" })).toBe(true);
		expect(outer).toHaveBeenCalledWith(expect.objectContaining({ key: "Enter" }));
		expect(box).toHaveValue("~/code/osa");
		expect(screen.queryByRole("listbox")).toBeNull();
		await waitFor(() => expect(changes).toHaveBeenLastCalledWith("/home/dev/code/osa"));

		type(box, "~/code/os");
		await screen.findByRole("listbox");
		fireEvent.keyDown(box, { key: "ArrowDown" });
		expect(fireEvent.keyDown(box, { key: "Enter" })).toBe(false);
		expect(box).toHaveValue("~/code/Mosaic");
	});

	it("asks the device only for the name you type, and narrows a whole answer without asking again", async () => {
		const source = sourceWith();
		const listFolder = vi.spyOn(source, "listFolder");
		const { box } = show({ device: DEVBOX, source });
		await ready();
		type(box, "~/code/D");
		const list = await screen.findByRole("listbox");
		expect(optionsOf(list)).toEqual(["~/code/DuskToolsgit · dev"]);
		expect(listFolder).toHaveBeenLastCalledWith(DEVBOX, "~/code", {
			files: false,
			hidden: true,
			filter: "d",
		});
		const asked = listFolder.mock.calls.length;
		type(box, "~/code/Du");
		await act(() => new Promise((done) => setTimeout(done, 300)));
		expect(optionsOf(screen.getByRole("listbox"))).toEqual(["~/code/DuskToolsgit · dev"]);
		expect(listFolder).toHaveBeenCalledTimes(asked);
	});

	it("asks again where a file goes, since a folder's listing left the files out", async () => {
		const source = sourceWith();
		const ui = createUiStore();
		const field = (wants: PathWants) => (
			<ServicesProvider source={source} ui={ui}>
				<PathField
					device={DEVBOX}
					value="~/bin/"
					onChange={() => {}}
					label="Path"
					title="Choose the agent"
					wants={wants}
					className="h-8"
				/>
			</ServicesProvider>
		);
		const listFolder = vi.spyOn(source, "listFolder");
		const { rerender } = render(field("folder"));
		await ready();
		act(() => screen.getByRole("textbox", { name: "Path" }).focus());
		await waitFor(() => expect(listFolder).toHaveBeenCalled());
		expect(screen.queryByRole("listbox")).toBeNull();
		rerender(field("file"));
		expect(optionsOf(await screen.findByRole("listbox"))).toEqual(["~/bin/agent-cli"]);
	});

	it("asks again after the folder browser closes, so a folder made there shows", async () => {
		const { box } = show({ device: DEVBOX });
		await ready();
		type(box, "~/code/");
		await screen.findByRole("listbox");
		fireEvent.click(browse());
		const dialog = within(await screen.findByRole("dialog"));
		await dialog.findByRole("option", { name: "notes" });
		fireEvent.click(dialog.getByRole("button", { name: "New folder" }));
		const name = dialog.getByRole("textbox", { name: "New folder's name" });
		fireEvent.change(name, { target: { value: "fresh" } });
		fireEvent.keyDown(name, { key: "Enter" });
		await dialog.findByRole("option", { name: "fresh" });
		fireEvent.click(dialog.getByRole("button", { name: "Cancel" }));
		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

		act(() => box.focus());
		await waitFor(() =>
			expect(optionsOf(screen.getByRole("listbox"))).toContain("~/code/fresh")
		);
		expect(box).toHaveValue("~/code/");
	});

	it("lists files where a file goes, and leaves a name on PATH as typed", async () => {
		const { box, changes } = show({ device: DEVBOX, wants: "file" });
		await ready();
		type(box, "~/bin/");
		expect(optionsOf(await screen.findByRole("listbox"))).toEqual(["~/bin/agent-cli"]);
		type(box, "agent-cli");
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(changes).toHaveBeenLastCalledWith("agent-cli");
	});
});

describe("Browse…", () => {
	it("opens the system picker on this Mac, at the folder the field holds", async () => {
		native.inApp = true;
		native.picked = "/Users/me/Code/scratch";
		const { box, changes } = show({ device: MAC, start: "/Users/me/Code" });
		await ready();
		fireEvent.click(browse());
		await waitFor(() => expect(box).toHaveValue("/Users/me/Code/scratch"));
		expect(native.pickPath).toHaveBeenCalledWith(
			"Choose the agent's folder",
			"/Users/me/Code",
			false
		);
		expect(changes).toHaveBeenLastCalledWith("/Users/me/Code/scratch");
		expect(screen.queryByRole("dialog")).toBeNull();
	});

	it("keeps the field as it was when you cancel the system picker", async () => {
		native.inApp = true;
		const { box } = show({ device: MAC, start: "/Users/me/Code" });
		await ready();
		fireEvent.click(browse());
		await waitFor(() => expect(native.pickPath).toHaveBeenCalled());
		expect(box).toHaveValue("/Users/me/Code");
	});

	it("opens MaxHQ's folder browser for another device and takes the folder you choose", async () => {
		const { box, changes } = show({ device: DEVBOX, start: "~/code" });
		await ready();
		fireEvent.click(browse());
		const dialog = within(
			await screen.findByRole("dialog", { name: "Choose a folder on devbox" })
		);
		const folders = dialog.getByRole("listbox", { name: "Folders" });
		await within(folders).findByRole("option", { name: /Mosaic/ });
		expect(optionsOf(folders)).toEqual([
			"DuskToolsgit · dev",
			"Mosaicgit · main",
			"notes",
			"scratch",
		]);
		const crumbs = dialog.getByRole("navigation", { name: "Folders above" });
		expect(within(crumbs).getByRole("button", { name: "code" })).toHaveAttribute(
			"aria-current",
			"location"
		);
		// With nothing picked, Choose takes the folder shown.
		expect(dialog.getByText("/home/dev/code")).toBeInTheDocument();

		fireEvent.doubleClick(within(folders).getByRole("option", { name: "notes" }));
		expect(await dialog.findByText("No folders here.")).toBeInTheDocument();
		fireEvent.click(dialog.getByRole("button", { name: "Up" }));
		const mosaic = await within(folders).findByRole("option", { name: /Mosaic/ });
		fireEvent.click(mosaic);
		expect(dialog.getByText("/home/dev/code/Mosaic")).toBeInTheDocument();
		fireEvent.click(dialog.getByRole("button", { name: "Choose" }));

		await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
		expect(box).toHaveValue("/home/dev/code/Mosaic");
		expect(changes).toHaveBeenLastCalledWith("/home/dev/code/Mosaic");
	});

	it("shows hidden folders when you ask", async () => {
		show({ device: DEVBOX });
		await ready();
		fireEvent.click(browse());
		const dialog = within(await screen.findByRole("dialog"));
		const folders = dialog.getByRole("listbox", { name: "Folders" });
		await within(folders).findByRole("option", { name: "code" });
		expect(within(folders).queryByRole("option", { name: ".config" })).toBeNull();
		fireEvent.click(dialog.getByRole("switch", { name: "Show hidden" }));
		expect(await within(folders).findByRole("option", { name: ".config" })).toBeInTheDocument();
	});

	it("makes a new folder there, and says why a name won't do", async () => {
		const { box } = show({ device: DEVBOX, start: "~/code" });
		await ready();
		fireEvent.click(browse());
		const dialog = within(await screen.findByRole("dialog"));
		await dialog.findByRole("option", { name: "notes" });
		fireEvent.click(dialog.getByRole("button", { name: "New folder" }));
		const name = dialog.getByRole("textbox", { name: "New folder's name" });
		fireEvent.change(name, { target: { value: "a/b" } });
		fireEvent.keyDown(name, { key: "Enter" });
		expect(await dialog.findByRole("alert")).toHaveTextContent(
			"A folder's name can't have a slash."
		);
		fireEvent.change(name, { target: { value: "fresh" } });
		fireEvent.keyDown(name, { key: "Enter" });
		expect(await dialog.findByRole("option", { name: "fresh" })).toHaveAttribute(
			"aria-selected",
			"true"
		);
		fireEvent.click(dialog.getByRole("button", { name: "Choose" }));
		await waitFor(() => expect(box).toHaveValue("/home/dev/code/fresh"));
	});

	it("waits for an update on a device whose maxd can't list folders, and suggests nothing", async () => {
		const source = sourceWith(olderDevbox);
		const listFolder = vi.spyOn(source, "listFolder");
		const { box } = show({ device: DEVBOX, source });
		const button = browse();
		expect(button).toHaveAttribute("aria-disabled", "true");
		act(() => button.focus());
		await waitFor(() =>
			expect(
				document.querySelector('[role="presentation"][data-open] > [data-open]')
			).toHaveTextContent("Update maxd on devbox to browse its folders")
		);
		fireEvent.click(button);
		expect(screen.queryByRole("dialog")).toBeNull();

		type(box, "~/code/");
		await act(() => new Promise((done) => setTimeout(done, 300)));
		expect(screen.queryByRole("listbox")).toBeNull();
		expect(listFolder).not.toHaveBeenCalled();
		expect(box).toHaveValue("~/code/");
	});
});

describe("drops from Finder", () => {
	const drop = (path: string) => act(async () => native.drop?.([path]));

	it("says what a drop will do while it's over the field", async () => {
		native.over = true;
		const mac = show({ device: MAC });
		await ready();
		expect(screen.getByRole("status")).toHaveTextContent("Drop to use this folder");
		expect(mac.box).toHaveClass("border-dashed", "border-primary");
		cleanup();

		const devbox = show({ device: DEVBOX });
		await ready();
		expect(screen.getByRole("alert")).toHaveTextContent("That folder is on this Mac.");
		expect(devbox.box).toHaveClass("border-dashed", "border-status-error");
	});

	it("takes a Mac folder on a field for this Mac", async () => {
		const { box, changes } = show({ device: MAC });
		await ready();
		await drop("/Users/me/code/shop/");
		await waitFor(() => expect(box).toHaveValue("/Users/me/code/shop"));
		expect(changes).toHaveBeenLastCalledWith("/Users/me/code/shop");
		expect(screen.queryByRole("status")).toBeNull();
	});

	it("takes the folder a file is in where a folder goes, and says so", async () => {
		const { box, changes } = show({ device: MAC, start: "~/Code" });
		await ready();
		// The field learns where home is from its first listing.
		await waitFor(() => expect(changes).toHaveBeenLastCalledWith("/Users/me/Code"));
		await drop("/Users/me/code/shop/README.md");
		await waitFor(() => expect(box).toHaveValue("/Users/me/code/shop"));
		expect(screen.getByRole("status")).toHaveTextContent(
			"Uses the folder it's in, ~/code/shop"
		);
	});

	it("says when it can't read a drop, and keeps what the field held", async () => {
		native.inApp = true;
		native.describePaths.mockResolvedValue({
			status: "error",
			error: { kind: "not_offered", path: "/Users/me/code/shop" },
		});
		const { box } = show({ device: MAC, start: "/Users/me/Code" });
		await ready();
		await drop("/Users/me/code/shop/");
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Couldn't read what you dropped. Try again."
		);
		expect(box).toHaveValue("/Users/me/Code");
		type(box, "/Users/me/Cod");
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("takes a file where a file goes, and refuses a folder there", async () => {
		const { box } = show({ device: MAC, wants: "file" });
		await ready();
		await drop("/usr/local/bin/");
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"Drop a file here, not a folder."
		);
		expect(box).toHaveValue("");
		await drop("/usr/local/bin/agent");
		await waitFor(() => expect(box).toHaveValue("/usr/local/bin/agent"));
		expect(screen.queryByRole("alert")).toBeNull();
	});

	it("says a Mac folder on another device's field is on this Mac, and offers that device's checkout", async () => {
		const { box, changes } = show({ device: DEVBOX, source: sourceWith(devboxComet) });
		await ready();
		await drop("/Users/me/Code/Github/Multi/Comet/");
		await waitFor(() =>
			expect(screen.getByRole("status")).toHaveTextContent(
				"That folder is on this Mac. devbox has a checkout of the same repo at /home/dev/code/Comet."
			)
		);
		expect(box).toHaveValue("");
		expect(box).toHaveClass("border-dashed", "border-status-error");
		expect(changes).not.toHaveBeenCalled();
		fireEvent.click(screen.getByRole("button", { name: "Use that" }));
		expect(box).toHaveValue("/home/dev/code/Comet");
		expect(changes).toHaveBeenLastCalledWith("/home/dev/code/Comet");
		expect(screen.queryByRole("status")).toBeNull();
	});

	it("offers to browse the other device when it has no checkout of the same repo", async () => {
		show({ device: DEVBOX });
		await ready();
		await drop("/Users/me/Desktop/");
		expect(await screen.findByRole("status")).toHaveTextContent("That folder is on this Mac.");
		expect(screen.queryByRole("button", { name: "Use that" })).toBeNull();
		fireEvent.click(browse("Browse devbox…"));
		expect(
			await screen.findByRole("dialog", { name: "Choose a folder on devbox" })
		).toBeInTheDocument();
	});
});
