// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import type { Agent, DeviceLink, TimelineItem } from "@maxhq/shared/bindings";
import { entities, link, workspace } from "@maxhq/shared/test/build";
import { applyUpdate, EMPTY_TIMELINE } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { ServicesProvider } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { Composer } from "@/composer/composer";
import { ComposerNativeProvider } from "@/composer/native";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import {
	AGENT,
	DEVICE,
	DONE,
	fakeNative,
	fakeSource,
	findTextbox,
	heldAgent,
	installBrowserFakes,
	textIn,
} from "@/composer/testing";

import { ItemBodyView } from "./item-row";

// The terminal module, as the menu reaches it.
const seam = vi.hoisted(() => ({
	pasteIntoDrawer: vi.fn<(agent: string, text: string) => void>(),
}));

vi.mock("@/terminal", () => ({
	LiveTerminal: () => null,
	useTerminalActions: () => ({
		sendInput: () => {},
		text: () => null,
		runningText: () => null,
		cellSize: () => ({ width: 8, height: 17 }),
		pasteIntoDrawer: seam.pasteIntoDrawer,
		showInDrawer: () => {},
	}),
}));

beforeAll(installBrowserFakes);
beforeEach(() => seam.pasteIntoDrawer.mockReset());
afterEach(() => {
	cleanup();
	clearKept();
	quoteInbox.take(AGENT);
	localStorage.clear();
});

const WITH_TERMINALS = { transfers: true, tunnels: true, workspaces: true, terminals: true };

/** The Mac's link, its maxd speaking protocol 45 with terminals unless a test says otherwise. */
const macLink = (over: Partial<DeviceLink> = {}): DeviceLink =>
	link({
		id: "mac",
		name: "Mac",
		device: DEVICE,
		protocol: 45,
		capabilities: WITH_TERMINALS,
		...over,
	});

const reply = (text: string): TimelineItem =>
	timelineItem(
		{ kind: "assistant_message", text, streaming: false },
		{ id: "m1", agent: AGENT, ordinal: 1 }
	);

const BASH = "Try this:\n\n```bash\npnpm test\n```\n";

/** One timeline item, with the agent's composer below it. */
const renderItem = (
	item: TimelineItem,
	{ links = [macLink()], agent = heldAgent() }: { links?: DeviceLink[]; agent?: Agent } = {}
) => {
	const fake = fakeSource({
		agent,
		links,
		more: entities({
			workspaces: [
				workspace({
					id: "w1",
					location: { type: "folder", path: "/Users/dev/code/example-app" },
				}),
			],
		}),
	});
	const timeline = createTimelineStore();
	timeline.setState(
		applyUpdate(EMPTY_TIMELINE, { type: "reset", seq: 1, items: [item], has_older: false }),
		true
	);
	const row: RowContext = {
		agent: AGENT,
		vendor: "claude",
		timeline,
		readOnly: false,
		remote: false,
		answer: async () => DONE,
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	const view = render(
		<ServicesProvider source={fake.source}>
			<RowProvider value={row}>
				<ItemBodyView item={item} />
			</RowProvider>
			<ComposerNativeProvider value={fakeNative().native}>
				<Composer agent={AGENT} device={DEVICE} queued={[]} timeline={timeline} />
			</ComposerNativeProvider>
			<ConfirmHost />
		</ServicesProvider>
	);
	return { ...fake, container: view.container };
};

/** The rendered code blocks, once the Markdown has drawn them. */
const findBlocks = async (container: HTMLElement) => {
	await waitFor(() => expect(container.querySelector("pre code")).not.toBeNull());
	// The view learns the Mac's link a beat after the first draw.
	await findTextbox();
};

/** Opens Run ▾ and gives its menu. */
const openRun = async () => {
	fireEvent.click(await screen.findByRole("button", { name: "Run this code" }));
	return screen.findByRole("menu");
};

const commandField = () =>
	screen.findByRole<HTMLTextAreaElement>("textbox", { name: "Command to run" });

describe("Run ▾ on a code block", () => {
	it("offers running with ! or !! and pasting into the terminal", async () => {
		renderItem(reply(BASH));

		const menu = await openRun();

		expect(
			within(menu)
				.getAllByRole("menuitem")
				.map((each) => each.textContent)
		).toEqual([
			"Run with !output goes to the agent",
			"Run privately with !!",
			"Paste into terminal",
		]);
		expect(within(menu).getByRole("separator")).toBeInTheDocument();
	});

	it("puts the command in the composer in shell mode, ready to edit, for Run with !", async () => {
		renderItem(reply(BASH));
		const textbox = await findTextbox();

		fireEvent.click(within(await openRun()).getByRole("menuitem", { name: /^Run with !/ }));

		const field = await commandField();
		expect(field.value).toBe("pnpm test");
		expect(screen.getByText("Shell")).toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Run" })).toBeEnabled();
		expect(textIn(textbox)).toBe("");
	});

	it("puts it in private mode for Run privately with !!", async () => {
		renderItem(reply(BASH));

		fireEvent.click(
			within(await openRun()).getByRole("menuitem", { name: "Run privately with !!" })
		);

		const field = await screen.findByRole<HTMLTextAreaElement>("textbox", {
			name: "Command to run privately",
		});
		expect(field.value).toBe("pnpm test");
		expect(screen.getByText("Private")).toBeInTheDocument();
		expect(screen.getByText("The agent won't see the output")).toBeInTheDocument();
	});

	it("types it into the agent's terminal drawer for Paste into terminal", async () => {
		renderItem(reply(BASH));

		fireEvent.click(
			within(await openRun()).getByRole("menuitem", { name: "Paste into terminal" })
		);

		expect(seam.pasteIntoDrawer).toHaveBeenCalledExactlyOnceWith(AGENT, "pnpm test");
		expect(screen.queryByRole("textbox", { name: "Command to run" })).toBeNull();
	});

	it("takes only the commands of a console block, without their $ prompts", async () => {
		renderItem(reply("```console\n$ pnpm install\nPackages: +12\n$ pnpm test\nok\n```\n"));

		fireEvent.click(within(await openRun()).getByRole("menuitem", { name: /^Run with !/ }));

		expect((await commandField()).value).toBe("pnpm install\npnpm test");
	});

	it.each(["sh", "zsh", "shell", "fish"])("is on a %s block", async (lang) => {
		renderItem(reply(`\`\`\`${lang}\nls\n\`\`\`\n`));

		expect(await screen.findByRole("button", { name: "Run this code" })).toBeInTheDocument();
	});

	it("isn't on code that isn't commands", async () => {
		const { container } = renderItem(reply("```ts\nconst a = 1;\n```\n"));
		await findBlocks(container);

		expect(screen.queryByRole("button", { name: "Run this code" })).toBeNull();
	});

	it("isn't on a message you sent", async () => {
		const { container } = renderItem(
			timelineItem(
				{ kind: "user_message", text: BASH, attachments: [], delivery: "sent" },
				{ id: "u1", agent: AGENT, ordinal: 1 }
			)
		);
		await findTextbox();
		await waitFor(() => expect(container).toHaveTextContent("pnpm test"));

		expect(screen.queryByRole("button", { name: "Run this code" })).toBeNull();
	});

	it("isn't on the reply of an agent you only watch", async () => {
		const { container } = renderItem(reply(BASH), {
			agent: heldAgent({ control: "observed" }),
		});
		await waitFor(() => expect(container.querySelector("pre code")).not.toBeNull());

		expect(screen.queryByRole("button", { name: "Run this code" })).toBeNull();
	});

	it("is hidden while the device's maxd has no terminals", async () => {
		const { container } = renderItem(reply(BASH), { links: [macLink({ protocol: 44 })] });
		await findBlocks(container);

		expect(screen.queryByRole("button", { name: "Run this code" })).toBeNull();
	});
});
