// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { type ReactNode, useEffect } from "react";
import { describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { panesOf } from "@/layout/panes";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { Markdown } from "./markdown";
import { LinkFolder } from "./markdown-link";

const row: RowContext = {
	agent: "a1",
	vendor: "claude",
	timeline: createTimelineStore(),
	readOnly: false,
	remote: false,
	answer: async () => ({ status: "ok", data: { status: "done" } }),
	subAgentsShown: false,
	toggleSubAgents: () => {},
};

const grabbed: { services: Services | null } = { services: null };
const Grab = () => {
	const services = useServices();
	useEffect(() => {
		grabbed.services = services;
	});
	return null;
};
const openFile = () =>
	vi.spyOn(panesOf(grabbed.services!), "openFile").mockImplementation(() => {});

const source = mockSource({ tick: false });

const tree = (text: string, wrap: (children: ReactNode) => ReactNode) => (
	<ServicesProvider source={source}>
		<Grab />
		{wrap(<Markdown text={text} />)}
	</ServicesProvider>
);

const show = (text: string, wrap: (children: ReactNode) => ReactNode = (c) => c) =>
	render(tree(text, wrap));

const inRow = (children: ReactNode) => <RowProvider value={row}>{children}</RowProvider>;

describe("a link in a reply", () => {
	it("opens a web page in the browser", () => {
		show("[Docs](https://example.com/docs) and <https://example.com>", inRow);
		const link = screen.getByText("Docs");
		expect(link.getAttribute("href")).toBe("https://example.com/docs");
		expect(link.getAttribute("target")).toBe("_blank");
		expect(screen.getByText("https://example.com").getAttribute("target")).toBe("_blank");
	});

	it.each([
		["[main](src/main.rs:12)", "main", "src/main.rs", { line: 12 }],
		["[lib](src/lib.rs#L40-L52)", "lib", "src/lib.rs", { line: 40 }],
		["[cfg](/srv/app/config.toml)", "cfg", "/srv/app/config.toml", {}],
		["[url](file:///srv/app/a%20b.ts:3)", "url", "/srv/app/a b.ts", { line: 3 }],
		["[win](windows.rs:7)", "win", "windows.rs", { line: 7 }],
	])("opens %s in the file viewer, on the agent's device", (text, name, path, options) => {
		show(text, inRow);
		const spy = openFile();
		fireEvent.click(screen.getByText(name));
		expect(spy).toHaveBeenCalledWith("a1", path, options);
	});

	it("says the line it opens at", () => {
		show("[main](src/main.rs:12)", inRow);
		expect(screen.getByText("main").title).toBe("Open src/main.rs at line 12");
	});

	it("reads a link in a Markdown file from the file's folder", () => {
		show("[Next](../guide/next.md)", (children) =>
			inRow(<LinkFolder value="/srv/app/docs/intro/">{children}</LinkFolder>)
		);
		const spy = openFile();
		fireEvent.click(screen.getByText("Next"));
		expect(spy).toHaveBeenCalledWith("a1", "/srv/app/docs/guide/next.md", {});
	});

	it.each([
		["a scheme MaxHQ doesn't open", "[app](vscode://file/x)", "app"],
		["a home path", "[rc](~/notes.txt)", "rc"],
	])("shows %s as plain text that says so", (_, text, name) => {
		show(text, inRow);
		const plain = screen.getByText(name);
		expect(plain.tagName).toBe("SPAN");
		expect(plain.title).toContain("MaxHQ can't open this link");
	});

	it("shows a file link as plain text where there's no agent to open it on", () => {
		show("[main](src/main.rs:12)");
		expect(screen.getByText("main").tagName).toBe("SPAN");
	});

	it("moves to a heading in the same message", () => {
		show("[Go](#setup-steps)\n\n## Setup steps\n\nText", inRow);
		const heading = screen.getByRole("heading", { name: "Setup steps" });
		heading.scrollIntoView = vi.fn();
		fireEvent.click(screen.getByText("Go"));
		expect(heading.scrollIntoView).toHaveBeenCalled();
	});

	it("shows an anchor with nothing to move to as plain text", () => {
		show("[Gone](#nowhere)\n\nText", inRow);
		expect(screen.getByText("Gone").tagName).toBe("SPAN");
	});

	it("turns an anchor into a link once its target streams in", async () => {
		const shown = show("[Later](#later)\n\nText", inRow);
		expect(screen.getByText("Later").tagName).toBe("SPAN");
		const first = screen.getByText("Later").closest("p");
		shown.rerender(tree("[Later](#later)\n\nText\n\n## Later", inRow));
		// The first block stays as it was, and its link finds the new heading.
		expect(screen.getAllByText("Later")[0]!.closest("p")).toBe(first);
		await waitFor(() => expect(screen.getByText("Later", { selector: "a" })).toBeTruthy());
	});
});
