// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { type Services, ServicesProvider, useServices } from "@/app/services";
import { mockSource } from "@/data/mock-source";
import { ComposerNativeProvider } from "@/composer/native";
import {
	checkerFor,
	editorOf,
	fakeNative,
	installBrowserFakes,
	pillsIn,
	underlined,
} from "@/composer/testing";
import { DispatcherSettingsPage } from "@/settings/dispatcher-settings";
import { settingsOf } from "@/settings/store";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { ANSWER_PILLS_SINCE, type Entity } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { CODEX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { OBJECT_MARK as MARK } from "@maxhq/shared/pills/draft";
import { NOW } from "@maxhq/shared/test/build";
import { createUiStore } from "@maxhq/shared/ui/store";

import { DispatchBox } from "./dispatcher";
import { DISPATCHER_ACCOUNT_KEY } from "./dispatcher-prefs";

const REQUEST =
	"Please make a cursor agent that will review all new PRs and PR updates from octocat on owner/repo, with a 15 minute debounce";

beforeAll(installBrowserFakes);

afterEach(() => {
	localStorage.clear();
	clearKept();
});

/** The box with the sample source, once the view has the Mac's dispatcher. */
const renderBox = async (
	extra?: ReactNode,
	native: ReturnType<typeof fakeNative> = fakeNative(),
	source: DataSource = mockSource({ now: () => NOW, tick: false })
) => {
	let services: Services | undefined;
	const Grab = () => {
		services = useServices();
		return null;
	};
	render(
		<FixedNow value={NOW}>
			<ServicesProvider source={source} ui={createUiStore()}>
				<ComposerNativeProvider value={native.native}>
					<Grab />
					<DispatchBox />
					<div data-extra>{extra}</div>
				</ComposerNativeProvider>
			</ServicesProvider>
		</FixedNow>
	);
	await waitFor(() =>
		expect(services?.view.getState().view.tables.dispatcher.has(MAC)).toBe(true)
	);
	const input = screen.getByRole("textbox", { name: "Ask the dispatcher" });
	const view = () => services!.view.getState().view;
	return { services: services!, input, view };
};

/** The sample source, with each entity of its snapshot passed through `change`. */
const changed = (change: (entity: Entity) => Entity) => {
	const source = mockSource({ now: () => NOW, tick: false });
	const snapshot = source.snapshot;
	return {
		...source,
		snapshot: async () => {
			const loaded = await snapshot();
			return { ...loaded, entities: loaded.entities.map(change) };
		},
	};
};

/** Puts text in the field as if typed. */
const type = (input: HTMLElement, text: string) =>
	act(() => {
		editorOf(input).commands.insertContent(text);
	});

const textIn = (input: HTMLElement) => editorOf(input).getText({ blockSeparator: "\n" });

/** Whether the field shows the whole draft, or keeps to one line. */
const fieldState = (input: HTMLElement) =>
	input.closest("[data-state]")?.getAttribute("data-state");

const ask = (input: HTMLElement, text: string) => {
	fireEvent.focus(input);
	type(input, text);
	fireEvent.keyDown(input, { key: "Enter" });
};

// The box's panel, and not the Dispatcher card a test renders beside it.
const panel = async () =>
	within(
		await waitFor(() => {
			const [shown, ...more] = screen
				.getAllByRole("region", { name: "Dispatcher" })
				.filter((region) => !region.closest("[data-extra]"));
			if (!shown || more.length > 0) throw new Error("No dispatcher panel");
			return shown;
		})
	);

/** Asks, answers the project question with Comet, and returns the plan. */
const planInComet = async (input: HTMLElement) => {
	ask(input, REQUEST);
	const shown = await panel();
	fireEvent.click(await shown.findByRole("radio", { name: /Comet/ }));
	fireEvent.click(shown.getByRole("button", { name: "Send answer" }));
	return within(await shown.findByRole("region", { name: "Plan" }));
};

describe("the dispatch box", () => {
	it("is a text box that sends on Enter and shows the reply's question", async () => {
		const { input, view } = await renderBox();
		expect(input).toHaveAttribute(
			"aria-placeholder",
			"Ask anything, or describe a task for any project"
		);
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();
		ask(input, REQUEST);
		const shown = await panel();
		expect(await shown.findByText(REQUEST)).toBeInTheDocument();
		expect(
			await shown.findByText("Which project should the agent work in?")
		).toBeInTheDocument();
		await waitFor(() => expect(textIn(input)).toBe(""));
		const agent = view().tables.dispatcher.get(MAC)?.agent;
		expect(agent && view().tables.agent.get(agent)?.title).toBe("Dispatcher");
		expect(shown.getByRole("button", { name: "Open as pane" })).toBeEnabled();
	});

	it("shows the plan in one line and opens an item to its filters and wait", async () => {
		const { input } = await renderBox();
		const plan = await planInComet(input);
		expect(
			plan.getByText(/^2 items: a Cursor agent in Comet on .+ and a task on .+ PR events$/)
		).toBeInTheDocument();
		expect(plan.getByText("Nothing runs until you confirm.")).toBeInTheDocument();
		// What each item does shows before you open it.
		expect(plan.getByText("Runs in Auto")).toBeInTheDocument();
		expect(plan.getByText("Review the PR changes in this batch.")).toBeInTheDocument();
		const task = plan.getByRole("button", { name: /^A task on/ });
		expect(task).toHaveAttribute("aria-expanded", "false");
		fireEvent.click(task);
		expect(task).toHaveAttribute("aria-expanded", "true");
		expect(plan.getByText("Only when")).toBeInTheDocument();
		expect(
			plan.getByText(/Waits 15 min after the last event, 2 h at most$/)
		).toBeInTheDocument();
		expect(plan.getByText(/^The new Cursor agent in Comet/)).toBeInTheDocument();
	});

	it("runs the plan on Enter and links to what it made", async () => {
		const { input, services, view } = await renderBox();
		await planInComet(input);
		expect(view().tables.agent.size).toBeGreaterThan(0);
		const before = [...view().tables.agent.values()].filter((a) => a.title === "PR reviews");
		expect(before).toHaveLength(0);

		fireEvent.keyDown(input, { key: "Enter" });
		const shown = await panel();
		const results = within(await shown.findByRole("region", { name: "Plan results" }));
		expect(results.getByText("The plan ran")).toBeInTheDocument();
		expect(results.getByText("Started")).toBeInTheDocument();
		expect(results.getByText("Task saved")).toBeInTheDocument();
		expect(shown.queryByRole("region", { name: "Plan" })).not.toBeInTheDocument();

		fireEvent.click(results.getByRole("button", { name: "Open agent" }));
		const selected = services.ui.getState().selectedAgent;
		expect(selected && view().tables.agent.get(selected)?.title).toBe("PR reviews");
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();

		fireEvent.focus(input);
		fireEvent.click((await panel()).getByRole("button", { name: "Open task" }));
		expect(settingsOf(services).getState()).toMatchObject({
			open: true,
			detail: { kind: "scheduled-tasks" },
		});
	});

	it("dismisses the plan on Esc, and starts a new conversation", async () => {
		const { input, view } = await renderBox();
		await planInComet(input);
		const first = view().tables.dispatcher.get(MAC)?.agent;

		fireEvent.keyDown(input, { key: "Escape" });
		await waitFor(() => expect(view().tables.dispatcher.get(MAC)?.plan).toBeNull());
		const shown = await panel();
		expect(shown.queryByRole("region", { name: "Plan" })).not.toBeInTheDocument();

		fireEvent.click(shown.getByRole("button", { name: "New conversation" }));
		expect(shown.getByText("What you ask next starts a new conversation.")).toBeInTheDocument();
		expect(shown.queryByRole("log", { name: "Conversation" })).not.toBeInTheDocument();

		ask(input, "Check on the reviewers");
		expect(await shown.findByText("Check on the reviewers")).toBeInTheDocument();
		const second = view().tables.dispatcher.get(MAC)?.agent;
		expect(second).toBeTruthy();
		expect(second).not.toBe(first);
		expect(view().tables.dispatcher_chat.has(first!)).toBe(true);

		// Esc with no plan closes the panel.
		fireEvent.keyDown(input, { key: "Escape" });
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();
	});
});

describe("the dispatch field", () => {
	it("opens to the whole draft while focused and keeps to one line when not", async () => {
		const { input } = await renderBox();
		expect(fieldState(input)).toBe("collapsed");
		fireEvent.focus(input);
		expect(fieldState(input)).toBe("expanded");
		type(input, REQUEST);
		fireEvent.blur(input);
		expect(fieldState(input)).toBe("collapsed");
		expect(textIn(input)).toBe(REQUEST);
	});

	it("breaks the line on Shift+Enter and sends the lines on Enter", async () => {
		const { input, view } = await renderBox();
		fireEvent.focus(input);
		type(input, "Check on the reviewers");
		fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
		type(input, "and the release");
		expect(textIn(input)).toBe("Check on the reviewers\nand the release");
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();

		fireEvent.keyDown(input, { key: "Enter" });
		const shown = await panel();
		expect(
			await shown.findByText(/^Check on the reviewers\s+and the release$/)
		).toBeInTheDocument();
		await waitFor(() => expect(textIn(input)).toBe(""));
		expect(view().tables.dispatcher.get(MAC)?.agent).toBeTruthy();
	});

	it("leaves spelling to the webview without native spell check", async () => {
		const { input } = await renderBox();
		expect(input).toHaveAttribute("spellcheck", "true");
	});

	it("underlines what the system's spell check flags, as the composer does", async () => {
		const native = fakeNative({ spelling: true });
		native.native.checkSpelling.mockImplementation(checkerFor(["reveiwers"]));
		const { input } = await renderBox(undefined, native);
		expect(input).toHaveAttribute("spellcheck", "false");
		type(input, "Check on the reveiwers");
		await waitFor(() => expect(underlined()).toEqual(["reveiwers"]));
	});

	it("leaves Enter to an IME composition", async () => {
		const { input } = await renderBox();
		fireEvent.focus(input);
		type(input, "Check on the reviewers");
		fireEvent.keyDown(input, { key: "Enter", keyCode: 229 });
		expect(textIn(input)).toBe("Check on the reviewers");
		expect(screen.queryByRole("region", { name: "Dispatcher" })).not.toBeInTheDocument();
	});
});

describe("the dispatcher settings", () => {
	it("takes the first Claude account, and the pick is the one the box uses", async () => {
		const { input } = await renderBox(<DispatcherSettingsPage />);
		const picker = screen.getByRole("combobox", { name: /^Runs as, Claude/ });
		fireEvent.click(picker);
		const options = await screen.findAllByRole("option");
		// A local model comes last, and only runs it when you pick it. A
		// cloud service goes by its name.
		expect(options.map((o) => o.textContent)).toEqual([
			expect.stringMatching(/^Claude/),
			expect.stringMatching(/^Codex/),
			"Local",
			"OpenRouter",
		]);
		const codex = options[1]!;
		fireEvent.pointerDown(codex, { pointerType: "mouse" });
		fireEvent.click(codex);
		await waitFor(() => expect(localStorage.getItem(DISPATCHER_ACCOUNT_KEY)).toBe(CODEX));
		expect(screen.getByRole("combobox", { name: /^Runs as, Codex/ })).toBeInTheDocument();

		ask(input, REQUEST);
		expect(await (await panel()).findByText(/^Dispatcher, as Codex/)).toBeInTheDocument();
	});

	it("acts directly by default, and saves the switch off on the Mac", async () => {
		const { services, view } = await renderBox(<DispatcherSettingsPage />);
		const save = vi.spyOn(services.source, "setDeviceSettings");
		const acts = screen.getByRole("switch", { name: "Act directly" });
		expect(acts).toBeChecked();
		expect(
			screen.getByText(/^The dispatcher changes settings, agents and tasks/)
		).toBeVisible();

		fireEvent.click(acts);
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({ device: MAC, dispatcher_acts: false })
			)
		);
		await waitFor(() =>
			expect(view().tables.device_settings.get(MAC)?.dispatcher_acts).toBe(false)
		);
		expect(acts).not.toBeChecked();
	});

	it("starts a fresh chat after 30 minutes by default, and saves another pause on the Mac", async () => {
		const { services, view } = await renderBox(<DispatcherSettingsPage />);
		const save = vi.spyOn(services.source, "setDeviceSettings");
		expect(screen.getByText(/^When you write again after this long/)).toBeVisible();
		fireEvent.click(
			screen.getByRole("combobox", { name: "New chat after a pause, 30 minutes" })
		);
		const options = await screen.findAllByRole("option");
		expect(options.map((o) => o.textContent)).toEqual([
			"15 minutes",
			"30 minutes",
			"1 hour",
			"2 hours",
			"4 hours",
			"Never",
		]);
		const hour = options[2]!;
		fireEvent.pointerDown(hour, { pointerType: "mouse" });
		fireEvent.click(hour);
		await waitFor(() =>
			expect(save).toHaveBeenCalledWith(
				expect.objectContaining({
					device: MAC,
					chat_gap_minutes: 60,
					dispatcher_acts: true,
				})
			)
		);
		await waitFor(() =>
			expect(view().tables.device_settings.get(MAC)?.chat_gap_minutes).toBe(60)
		);
		expect(
			screen.getByRole("combobox", { name: "New chat after a pause, 1 hour" })
		).toBeInTheDocument();
	});

	it("greys out the pause against a maxd too old for it", async () => {
		const source = changed((entity) =>
			entity.entity === "device_settings" && entity.device === MAC
				? { ...entity, chat_gap_minutes: undefined }
				: entity
		);
		await renderBox(<DispatcherSettingsPage />, undefined, source);
		expect(screen.getByRole("combobox", { name: /^New chat after a pause/ })).toBeDisabled();
		expect(screen.getByText(/maxd is too old for this\. Update it first\.$/)).toBeVisible();
		expect(screen.getByRole("switch", { name: "Act directly" })).toBeEnabled();
	});
});

describe("pills in a request", () => {
	/** The sample source, with the Mac's maxd speaking `protocol`. */
	const speaking = (protocol: number) =>
		changed((entity) =>
			entity.entity === "device_link" && entity.device === MAC
				? { ...entity, protocol }
				: entity
		);

	/** Names "Sidebar sort order" in a request to a Mac on `protocol`, and returns what went. */
	const askWithPill = async (protocol: number) => {
		const source = speaking(protocol);
		const asked = vi.spyOn(source, "askDispatcher");
		const { input } = await renderBox(undefined, undefined, source);
		fireEvent.focus(input);
		type(input, "Review @sidebar");
		fireEvent.click(await screen.findByRole("option", { name: /Sidebar sort order/ }));
		await waitFor(() => expect(pillsIn(input)).toHaveLength(1));
		fireEvent.keyDown(input, { key: "Enter" });
		await waitFor(() => expect(asked).toHaveBeenCalledTimes(1));
		await waitFor(() => expect(textIn(input)).toBe(""));
		const [device, text, , , refs] = asked.mock.calls[0]!;
		return { device, text, refs };
	};

	it("sends an agent named in a request as a pill", async () => {
		const { device, text, refs } = await askWithPill(ANSWER_PILLS_SINCE);
		expect(device).toBe(MAC);
		expect(text).toBe(`Review ${MARK}`);
		expect(refs).toEqual([
			expect.objectContaining({ type: "agent", title: "Sidebar sort order" }),
		]);
	});

	it("sends the pill as its words to a maxd too old for pills", async () => {
		const { text, refs } = await askWithPill(ANSWER_PILLS_SINCE - 1);
		expect(text).toBe("Review [Agent: Sidebar sort order]");
		expect(refs).toEqual([]);
	});
});
