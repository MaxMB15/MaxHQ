// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import type { AttachmentRef, InlineRef, TimelineItem } from "@maxhq/shared/bindings";
import { FixedNow } from "@maxhq/shared/lib/clock";
import {
	type ClipboardFragment,
	decodeClipboardHtml,
	decodeFragment,
	DRAFT_CLIPBOARD_MIME,
} from "@maxhq/shared/pills/clipboard";
import {
	type AgentRef,
	type GitItemRef,
	OBJECT_MARK,
	type QuoteRef,
} from "@maxhq/shared/pills/draft";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { ItemBodyView } from "./item-row";

const hooks = vi.hoisted(() => ({
	jumpToQuote: vi.fn<(quote: QuoteRef) => void>(),
	openAgent: vi.fn<(services: unknown, id: string) => void>(),
}));

vi.mock("@/agent-view/jump", () => ({ jumpToQuote: hooks.jumpToQuote }));

vi.mock("@/app/open-agent", async (load) => ({
	...(await load<typeof import("@/app/open-agent")>()),
	openAgent: hooks.openAgent,
}));

afterEach(() => {
	hooks.jumpToQuote.mockReset();
	hooks.openAgent.mockReset();
	window.getSelection()?.removeAllRanges();
});

const M = OBJECT_MARK;

const show = (item: TimelineItem, row: Partial<RowContext> = {}) => {
	const value: RowContext = {
		agent: "a1",
		vendor: "claude",
		timeline: createTimelineStore(),
		readOnly: false,
		remote: false,
		answer: async () => ({ status: "ok", data: { status: "done" } }),
		subAgentsShown: false,
		toggleSubAgents: () => {},
		...row,
	};
	const wrap = (children: ReactNode) => (
		<ServicesProvider source={mockSource({ now: () => NOW, tick: false })}>
			<FixedNow value={NOW}>
				<RowProvider value={value}>{children}</RowProvider>
			</FixedNow>
		</ServicesProvider>
	);
	return render(wrap(<ItemBodyView item={item} />));
};

const sent = (text: string, attachments: AttachmentRef[], refs?: InlineRef[]) =>
	timelineItem(
		{ kind: "user_message", text, attachments, ...(refs ? { refs } : {}), delivery: "sent" },
		{ id: "item-sent", status: "completed", started_at: NOW - 4000, completed_at: NOW }
	);

const file = (
	name: string,
	kind: AttachmentRef["kind"],
	thumbnail: string | null = null
): AttachmentRef => ({
	id: `att-${name}`,
	name,
	kind,
	bytes: 4096,
	mime: null,
	path: `/staged/${name}`,
	thumbnail,
});

const placed = (attachment: AttachmentRef): InlineRef => ({
	type: "attachment",
	id: attachment.id,
});

// "HDR tone mapping" is one of the sample build's agents.
const quote = (agent = "agent-hdr"): QuoteRef => ({
	type: "quote",
	agent,
	agent_title: "HDR tone mapping",
	item: "agent-hdr:1",
	text: "the highlights get clipped",
	position: 0,
	source: "reply",
	from: null,
});

const agent = (id = "agent-hdr", title = "HDR tone mapping"): AgentRef => ({
	type: "agent",
	agent: id,
	title,
	vendor: "codex",
	device: "dev-mac",
	excerpt: null,
});

const pr = (number: number): GitItemRef => ({
	type: "git_item",
	repo: "owner/repo",
	number,
	kind: "pull_request",
	title: "Cache usage between polls",
	state: "open",
	url: `https://github.com/owner/repo/pull/${number}`,
	head: "usage-cache",
	base: "main",
	body: "",
	checked_at: NOW,
});

const PREVIEW = "data:image/png;base64,iVBORw0KGgo=";
const SHOT = file("shot.png", "image", PREVIEW);
const SPEC = file("spec.pdf", "pdf");

const textOf = () => screen.getByText(/^Compare/);

const pillsIn = (element: HTMLElement) =>
	Array.from(element.querySelectorAll<HTMLElement>("[data-family]"));

// The sample agents arrive a moment after the view starts.
const loaded = () =>
	waitFor(() =>
		expect(screen.getByRole("button", { name: /^Agent HDR tone mapping/ })).toHaveAttribute(
			"data-state",
			"ready"
		)
	);

describe("a message you sent", () => {
	it("shows its pills where you put them, in order", async () => {
		show(
			sent(
				`Compare ${M} with ${M}, ask ${M} about #412 and #7.`,
				[SHOT],
				[placed(SHOT), quote(), agent(), pr(412)]
			)
		);
		await loaded();
		const text = textOf();
		expect(pillsIn(text).map((pill) => pill.getAttribute("aria-label"))).toEqual([
			expect.stringMatching(/^Image 1, shot\.png/),
			expect.stringMatching(/^Quote from HDR tone mapping, “the highlights get clipped”/),
			expect.stringMatching(/^Agent HDR tone mapping, Codex/),
			"Pull request 412, Cache usage between polls, open",
		]);
		expect(text).toHaveTextContent(/and #7\.$/);
		expect(text.textContent).not.toContain(M);
	});

	it("opens a pull request on GitHub from its #N", () => {
		show(sent("Compare it with #412", [], [pr(412)]));
		const link = screen.getByRole("link", { name: /^Pull request 412/ });
		expect(link).toHaveAttribute("href", "https://github.com/owner/repo/pull/412");
		expect(link).toHaveAttribute("target", "_blank");
		expect(link).toHaveTextContent("#412 Cache usage between polls");
	});

	it("numbers each placed picture and PDF to match its pill, and leaves out an agent's excerpt", () => {
		const excerpt = file("agent-old-meter.md", "file");
		const loose = file("loose.png", "image");
		show(
			sent(
				`Compare ${M} with ${M} and ask ${M}`,
				[SHOT, SPEC, excerpt, loose],
				[
					placed(SHOT),
					placed(SPEC),
					{ ...agent("agent-old", "Old meter"), excerpt: excerpt.id },
				]
			)
		);
		expect(screen.getByRole("button", { name: "Show shot.png full size" })).toBeInTheDocument();
		expect(screen.getByText("Image 1")).toBeInTheDocument();
		expect(screen.getByText("PDF 1, 4 KB")).toBeInTheDocument();
		// A file you didn't place in the text shows the old way, with no number.
		expect(
			screen.getByRole("button", { name: "Show loose.png full size" })
		).toBeInTheDocument();
		expect(screen.queryByText("Image 2")).toBeNull();
		expect(screen.queryByText("agent-old-meter.md")).toBeNull();
	});

	it("shows a pill whose source is gone as unresolved, with its label", async () => {
		show(
			sent(
				`Compare ${M} with ${M} and ${M}`,
				[],
				[agent(), agent("agent-old", "Old meter"), quote("agent-old")]
			)
		);
		await loaded();
		const [, gone, quoted] = pillsIn(textOf());
		expect(gone).toHaveAttribute("data-state", "unresolved");
		expect(gone).toHaveTextContent("Old meter");
		expect(gone).toHaveAccessibleName(/, gone$/);
		expect(quoted).toHaveAttribute("data-state", "unresolved");
		expect(quoted).toHaveTextContent("“the highlights get clipped”");
		fireEvent.click(gone!);
		fireEvent.click(quoted!);
		expect(hooks.openAgent).not.toHaveBeenCalled();
		expect(hooks.jumpToQuote).not.toHaveBeenCalled();
	});

	it("does what the composer does on a click", async () => {
		show(sent(`Compare ${M} with ${M}, ask ${M}`, [SHOT], [placed(SHOT), quote(), agent()]));
		await loaded();
		const [image, quoted, agentPill] = pillsIn(textOf());
		fireEvent.click(quoted!);
		expect(hooks.jumpToQuote).toHaveBeenCalledWith(quote());
		fireEvent.keyDown(quoted!, { key: "Enter" });
		expect(hooks.jumpToQuote).toHaveBeenCalledTimes(2);
		fireEvent.click(agentPill!);
		expect(hooks.openAgent).toHaveBeenCalledWith(expect.anything(), "agent-hdr");
		fireEvent.click(image!);
		const viewer = await screen.findByRole("dialog");
		expect(within(viewer).getByText("shot.png")).toBeInTheDocument();
	});

	it("copies its pills in the plain form another app reads", async () => {
		show(
			sent(
				`Compare ${M} with ${M}, ask ${M} about #412.`,
				[SHOT],
				[placed(SHOT), quote(), agent(), pr(412)]
			)
		);
		await loaded();
		const text = textOf();
		const range = document.createRange();
		range.selectNodeContents(text);
		window.getSelection()?.addRange(range);
		const copied = new Map<string, string>();
		fireEvent.copy(text, {
			clipboardData: { setData: (type: string, value: string) => copied.set(type, value) },
		});
		expect(copied.get("text/plain")).toBe(
			'Compare [Image 1: shot.png] with "the highlights get clipped", ask [Agent: HDR tone mapping] about #412.'
		);
	});

	describe("copied for a composer", () => {
		/** Copies `range` from the message's text, and gives what the clipboard got. */
		const copyOf = (text: HTMLElement, range: Range) => {
			window.getSelection()?.addRange(range);
			const copied = new Map<string, string>();
			fireEvent.copy(text, {
				clipboardData: {
					setData: (type: string, value: string) => copied.set(type, value),
				},
			});
			return copied;
		};

		const fragmentIn = (copied: Map<string, string>) =>
			decodeFragment(copied.get(DRAFT_CLIPBOARD_MIME));

		/** The first text inside a pill, to start or end a selection in it. */
		const labelIn = (pill: HTMLElement) => {
			const walker = document.createTreeWalker(pill, NodeFilter.SHOW_TEXT);
			return walker.nextNode()!;
		};

		const shotFile = (found: ClipboardFragment["files"][number]["found"]) => ({
			key: `sent-${SHOT.id}`,
			name: "shot.png",
			kind: "image",
			source: null,
			ref: null,
			device: null,
			found,
		});

		it("carries its pills, and where the agent has each file", async () => {
			show(
				sent(
					`Compare ${M} with ${M}, ask ${M} about #412 and #412.`,
					[SHOT],
					[placed(SHOT), quote(), { ...agent(), excerpt: "att-excerpt" }, pr(412)]
				),
				{ agent: "agent-hdr" }
			);
			await loaded();
			const text = textOf();
			const range = document.createRange();
			range.selectNodeContents(text);
			const copied = copyOf(text, range);
			const plain =
				'Compare [Image 1: shot.png] with "the highlights get clipped", ask [Agent: HDR tone mapping] about #412 and #412.';
			expect(copied.get("text/plain")).toBe(plain);
			const fragment = fragmentIn(copied);
			expect(fragment).toEqual({
				version: 1,
				origin: "sent",
				segments: [
					{ type: "text", text: "Compare " },
					{ type: "pill", pill: { type: "attachment", key: `sent-${SHOT.id}` } },
					{ type: "text", text: " with " },
					{ type: "pill", pill: quote() },
					{ type: "text", text: ", ask " },
					{ type: "pill", pill: agent() },
					{ type: "text", text: " about #412 and #412." },
				],
				files: [shotFile({ agent: "agent-hdr", device: "dev-mac", path: SHOT.path })],
				gitItems: [pr(412)],
			});
			// The HTML beside it shows the same words, and keeps no file.
			const html = decodeClipboardHtml(copied.get("text/html"));
			expect(html?.files).toEqual([]);
			expect(html?.segments[0]).toEqual({
				type: "text",
				text: "Compare [Image 1: shot.png] with ",
			});
		});

		it("keeps only the text a selection holds, and each pill it touches whole", async () => {
			show(
				sent(
					`Compare ${M} with ${M}, ask ${M} about #412.`,
					[SHOT],
					[placed(SHOT), quote(), agent(), pr(412)]
				),
				{ agent: "agent-hdr" }
			);
			await loaded();
			const text = textOf();
			const [, quoted] = pillsIn(text);
			const range = document.createRange();
			range.setStart(text.firstChild!, 3);
			range.setEnd(labelIn(quoted!), 2);
			const copied = copyOf(text, range);
			expect(copied.get("text/plain")).toBe(
				'pare [Image 1: shot.png] with "the highlights get clipped"'
			);
			expect(fragmentIn(copied)?.segments).toEqual([
				{ type: "text", text: "pare " },
				{ type: "pill", pill: { type: "attachment", key: `sent-${SHOT.id}` } },
				{ type: "text", text: " with " },
				{ type: "pill", pill: quote() },
			]);

			// A selection that starts inside a pull request's pill takes it whole too.
			window.getSelection()?.removeAllRanges();
			const link = screen.getByRole("link", { name: /^Pull request 412/ });
			const tail = document.createRange();
			tail.setStart(labelIn(link), 1);
			tail.setEnd(text.lastChild!, 1);
			const again = copyOf(text, tail);
			expect(again.get("text/plain")).toBe("#412.");
			expect(fragmentIn(again)).toMatchObject({
				segments: [{ type: "text", text: "#412." }],
				files: [],
				gitItems: [pr(412)],
			});
		});

		it("leaves a copy of plain words alone", async () => {
			show(sent(`Compare ${M} with this`, [SHOT], [placed(SHOT)]), { agent: "agent-hdr" });
			const text = textOf();
			const range = document.createRange();
			range.setStart(text.lastChild!, 1);
			range.setEnd(text.lastChild!, 5);
			window.getSelection()?.addRange(range);
			const setData = vi.fn();
			fireEvent.copy(text, { clipboardData: { setData } });
			expect(setData).not.toHaveBeenCalled();
		});

		it("names no place for a file when the agent's device isn't known", () => {
			show(sent(`Compare ${M}`, [SHOT], [placed(SHOT)]), { agent: "agent-gone" });
			const text = textOf();
			const range = document.createRange();
			range.selectNodeContents(text);
			const fragment = fragmentIn(copyOf(text, range));
			expect(fragment?.files).toEqual([shotFile(null)]);
		});
	});

	it("looks the way it always did when it has no refs", () => {
		show(sent("Like this", [SHOT, SPEC]));
		const text = screen.getByText("Like this");
		expect(pillsIn(text)).toEqual([]);
		expect(text.childNodes).toHaveLength(1);
		expect(screen.getByRole("button", { name: "Show shot.png full size" })).toBeInTheDocument();
		expect(screen.queryByText("Image 1")).toBeNull();
		expect(screen.getByText("PDF, 4 KB")).toBeInTheDocument();
		const range = document.createRange();
		range.selectNodeContents(text);
		window.getSelection()?.addRange(range);
		const setData = vi.fn();
		fireEvent.copy(text, { clipboardData: { setData } });
		expect(setData).not.toHaveBeenCalled();
	});
});
