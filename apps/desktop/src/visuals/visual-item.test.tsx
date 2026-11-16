// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider } from "@/app/services";
import { installBrowserFakes } from "@/composer/testing";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { mockSource } from "@/data/mock-source";
import { panesOf } from "@/layout/panes";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import type { ItemBody, TimelineItem } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { agent, NOW } from "@maxhq/shared/test/build";
import { applyUpdate } from "@maxhq/shared/timeline/apply";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";
import { toVisualFence } from "@maxhq/shared/visuals/fence";
import { VISUAL_MIME } from "@maxhq/shared/visuals/transfer";

import { ItemBodyView } from "@/agent-view/item-row";
import { menuEntries } from "./actions";
import { chatActions, writeVisual } from "./carry";
import { pagesOf } from "./pages";

beforeAll(installBrowserFakes);
afterEach(() => {
	quoteInbox.take("a1");
	quoteInbox.take("a2");
});

const visualBody = (id: string, replaces: string | null = null): ItemBody => ({
	kind: "agent_visual",
	title: "Weekly usage",
	attachment: {
		id: `att-${id}`,
		name: "visual-weekly-usage.html",
		kind: "file",
		bytes: 900,
		mime: "text/html",
		path: `/attachments/${id}.html`,
		thumbnail: null,
	},
	replaces,
});

const at = (body: ItemBody, id: string, ordinal: number) =>
	timelineItem(body, { id, ordinal, turn: "t1", completed_at: NOW });

/** A source whose reads of a page never finish unless a test says otherwise. */
const sourceWith = (media: DataSource["media"] = () => new Promise(() => {})): DataSource => ({
	...mockSource({ now: () => NOW, tick: false }),
	media,
});

const show = (items: TimelineItem[], shown: TimelineItem, source = sourceWith()) => {
	const timeline = createTimelineStore();
	timeline.setState(
		applyUpdate(timeline.getState(), { type: "reset", seq: 1, items, has_older: false })
	);
	const row: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline,
		readOnly: false,
		remote: false,
		answer: async () => ({ status: "ok", data: { status: "done" } }),
		subAgentsShown: false,
		toggleSubAgents: () => {},
	};
	const wrap = (children: ReactNode) => (
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<RowProvider value={row}>{children}</RowProvider>
			</FixedNow>
		</ServicesProvider>
	);
	return render(wrap(<ItemBodyView item={shown} />));
};

describe("the visual on the timeline", () => {
	it("keeps its row while the page is read", () => {
		const item = at(visualBody("v1"), "v1", 0);
		show([item], item);
		expect(screen.getByText("Weekly usage")).toBeInTheDocument();
		expect(screen.getByText("Showed")).toBeInTheDocument();
		expect(document.querySelector("iframe")).toBeNull();
	});

	it("says why the page couldn't be read", async () => {
		const item = at(visualBody("v2"), "v2", 0);
		show(
			[item],
			item,
			sourceWith(async () => ({
				status: "ok",
				data: { kind: "refused", outside: false, message: "Not in the agent's folders" },
			}))
		);
		expect(await screen.findByText("Not in the agent's folders")).toBeInTheDocument();
	});

	it("draws the page as a card with its version", () => {
		const first = at(visualBody("v3"), "v3", 0);
		const second = at(visualBody("v4", "v3"), "v4", 1);
		const source = sourceWith();
		pagesOf(source).put("att-v4", "<p>Weekly</p>");
		show([first, second], second, source);
		expect(document.querySelector("iframe")).not.toBeNull();
		expect(screen.getByText("v2")).toBeInTheDocument();
	});

	it("folds a replaced visual to a line, and Show draws it again", () => {
		const first = at(visualBody("v5"), "v5", 0);
		const second = at(visualBody("v6", "v5"), "v6", 1);
		const source = sourceWith();
		pagesOf(source).put("att-v5", "<p>Older</p>");
		show([first, second], first, source);
		expect(
			screen.getByText("Weekly usage, first version · replaced by the one below")
		).toBeInTheDocument();
		expect(document.querySelector("iframe")).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Show" }));
		expect(document.querySelector("iframe")).not.toBeNull();
	});

	it("hides a reply with no words in a turn that showed a visual", () => {
		const reply = at({ kind: "assistant_message", text: " ", streaming: false }, "r1", 1);
		const { container } = show([at(visualBody("v7"), "v7", 0), reply], reply);
		expect(container).toBeEmptyDOMElement();
		const alone = at({ kind: "assistant_message", text: "", streaming: true }, "r2", 0);
		show([alone], alone);
		expect(screen.getByText("Writing")).toBeInTheDocument();
	});
});

describe("a visual on its way to a chat", () => {
	const visual = { html: "<p>Weekly</p>", title: "Weekly usage", from: "Fix the build" };

	/** Services whose window shows `panes`, with `active` focused. */
	const servicesWith = (panes: string[], active: string | null) => {
		const agents = new Map(
			["a1", "a2", "a3"].map((id) => [
				id,
				agent({ id, title: `Agent ${id}`, control: id === "a3" ? "observed" : "held" }),
			])
		);
		const services = {
			source: sourceWith(),
			view: { getState: () => ({ view: { tables: { agent: agents } } }) },
			ui: {},
		} as unknown as Services;
		vi.spyOn(panesOf(services), "api").mockReturnValue({
			panels: panes.map((id) => ({ id })),
			activePanel: active === null ? undefined : { id: active },
		} as never);
		return services;
	};

	it("adds it to its own agent's chat, and lists the other panes' agents", () => {
		const actions = chatActions(servicesWith(["a1", "a2", "a3"], "a2"), visual, "a1");
		expect(actions.map((action) => action.label)).toEqual(["Add to chat", "Add to chat in"]);
		actions[0]!.run();
		expect(quoteInbox.take("a1")).toEqual([{ visual, focus: true }]);

		const into = actions[1]!.entries!();
		expect(into.map((each) => each.label)).toEqual(["Agent a2"]);
		into[0]!.run();
		expect(quoteInbox.take("a2")).toEqual([{ visual, focus: false }]);
		expect(menuEntries(actions)).toEqual([
			{ kind: "item", text: "Add to chat", action: expect.any(Function) },
			{
				kind: "submenu",
				text: "Add to chat in",
				entries: [{ kind: "item", text: "Agent a2", action: expect.any(Function) }],
			},
		]);
	});

	it("goes to the focused pane's agent when the reply's agent takes none", () => {
		const actions = chatActions(servicesWith(["a2"], "a2"), visual, null);
		actions[0]!.run();
		expect(quoteInbox.take("a2")).toEqual([{ visual, focus: true }]);
		expect(chatActions(servicesWith(["a3"], "a3"), visual, null)[0]!.label).toBe(
			"Add to chat in"
		);
	});

	it("writes three types for a copy or a drag", () => {
		const data = new Map<string, string>();
		writeVisual({ setData: (type, value) => void data.set(type, value) }, visual);
		expect(data.get("text/plain")).toBe(toVisualFence(visual));
		expect(data.get("application/x-maxhq-draft")).toContain("Weekly usage");
		expect(data.get("text/html")).toContain("Weekly usage");
		expect(JSON.parse(data.get(VISUAL_MIME) ?? "")).toEqual(visual);
	});
});
