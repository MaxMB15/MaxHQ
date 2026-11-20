// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { ServicesProvider } from "@/app/services";
import { ConfirmHost } from "@/components/confirm";
import { ComposerNativeProvider } from "@/composer";
import { fakeNative } from "@/composer/testing";
import { mockSource } from "@/data/mock-source";
import { CODEX, GEMINI_ACCOUNT, mockEntities } from "@maxhq/shared/data/mock-view";
import type { ActionResult, DataSource } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { type Agent, type ModelChoice, TAKE_OVER_SINCE } from "@maxhq/shared/bindings";
import { agent as agentOf, MIN, NOW } from "@maxhq/shared/test/build";
import { pullLink } from "@maxhq/shared/test/pulls";
import { createUiStore } from "@maxhq/shared/ui/store";

import { AgentView } from "./agent-view";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { OutsideBanner, resumeCommand } from "./outside-banner";
import { canResume, ResumeBanner } from "./resume-banner";
import { clearSnapshots } from "./timeline-list";

// jsdom lays nothing out, and the list draws no rows into a window of no
// height. Give every element a size.
beforeAll(() => {
	vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(600);
	vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(800);
});
afterAll(() => vi.restoreAllMocks());

afterEach(() => {
	clearKept();
	clearSnapshots();
});

const sample = (id: string, over: Partial<Agent> = {}): Agent => {
	const found = mockEntities(NOW).find((e) => e.entity === "agent" && e.id === id);
	if (found?.entity !== "agent") throw new Error(`No sample agent ${id}`);
	return { ...found, ...over };
};

// The sample source, with one agent changed in the view as well, and the
// models its account lists when given.
const sourceWith = (agent: Agent | null, models: readonly ModelChoice[] = []) => {
	const source = mockSource({ now: () => NOW, tick: false });
	if (!agent) return source;
	const snapshot = source.snapshot;
	const changed: DataSource["snapshot"] = async () => {
		const state = await snapshot();
		return {
			...state,
			entities: state.entities.map((e) => {
				if (e.entity === "agent" && e.id === agent.id) return { entity: "agent", ...agent };
				if (e.entity === "account" && e.id === agent.account && models.length > 0)
					return { ...e, models: [...models] };
				return e;
			}),
		};
	};
	return Object.assign(source, { snapshot: changed });
};

const show = (
	agent: Agent,
	element = <AgentView agent={agent} />,
	models: readonly ModelChoice[] = []
) => {
	const source = sourceWith(agent, models);
	const continueHere = vi.spyOn(source, "continueHere");
	render(
		<ServicesProvider source={source}>
			<FixedNow value={NOW}>
				<div style={{ height: 600 }}>{element}</div>
			</FixedNow>
		</ServicesProvider>
	);
	return { source, continueHere };
};

describe("agent view", () => {
	it("shows the header, the timeline and, for a session in a terminal, the banner", async () => {
		const agent = sample("agent-release", { control: "observed" });
		show(agent);
		const view = screen.getByRole("region", { name: agent.title });
		expect(within(view).getByRole("heading", { level: 1 })).toHaveTextContent(agent.title);
		expect(
			await within(view).findByText(/Go through the release checklist/)
		).toBeInTheDocument();
		expect(
			within(view).getByRole("region", { name: "Running in a terminal" })
		).toBeInTheDocument();
		// The composer shows nothing for it, not even the message it has queued.
		expect(
			within(view).queryByRole("list", { name: "Queued messages" })
		).not.toBeInTheDocument();
	});

	it("puts the composer under a session MaxHQ holds, with the queued message in it", async () => {
		const agent = sample("agent-release", { control: "held" });
		show(agent);
		const queued = await screen.findByRole("list", { name: "Queued messages" });
		expect(within(queued).getByRole("listitem", { name: "Queued message" })).toHaveTextContent(
			"Once the tests pass, bump everything to 0.3.0."
		);
		expect(
			screen.queryByRole("region", { name: "Running in a terminal" })
		).not.toBeInTheDocument();
	});

	it("lets the whole pane take file drops for the composer", async () => {
		const agent = sample("agent-release", { control: "held" });
		const native = fakeNative();
		show(
			agent,
			<ComposerNativeProvider value={native.native}>
				<AgentView agent={agent} />
			</ComposerNativeProvider>
		);
		await screen.findByRole("list", { name: "Queued messages" });
		const pane = screen.getByRole("region", { name: agent.title });
		await waitFor(() =>
			expect(native.native.registerDropTarget).toHaveBeenLastCalledWith(
				pane,
				expect.anything()
			)
		);
		// Never the composer's own box, not even before the pane mounts. The
		// card's answer field takes its own, to say a file can't go in it.
		const targets = native.native.registerDropTarget.mock.calls
			.map(([element]) => element)
			.filter((element) => !element.matches(".answer-field"));
		expect(targets).toEqual(targets.map(() => pane));
	});

	it("says so when the session has nothing in it yet", async () => {
		const agent = agentOf({ id: "agent-new", control: "held", title: "New agent" });
		const source = mockSource({ now: () => NOW, tick: false });
		vi.spyOn(source, "openTimeline").mockImplementation(async (_agent, onUpdate) => {
			onUpdate({ type: "reset", seq: 1, items: [], has_older: false }, false);
			return () => {};
		});
		render(
			<ServicesProvider source={source}>
				<div style={{ height: 600 }}>
					<AgentView agent={agent} />
				</div>
			</ServicesProvider>
		);
		expect(
			await screen.findByText("No messages yet. Send one below to start.")
		).toBeInTheDocument();
	});

	it("puts the port prompts under the header, above the conversation", async () => {
		const agent = sample("agent-gtfs", { control: "held" });
		show(agent);
		const ports = await screen.findByRole("region", { name: "Ports" });
		expect(ports).toHaveTextContent("6 more ports");
		const header = screen.getByRole("heading", { level: 1 });
		const conversation = screen.getByRole("region", { name: "Conversation" });
		const composer = await screen.findByRole("textbox", { name: `Message ${agent.title}` });
		const follows = (a: Element, b: Element) =>
			(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
		expect(follows(header, ports)).toBe(true);
		expect(follows(ports, conversation)).toBe(true);
		expect(follows(conversation, composer)).toBe(true);
	});
});

describe("agent header", () => {
	const MODELS: ModelChoice[] = [
		{ id: "opus", label: "Opus 4.8", default: true },
		{ id: "sonnet", label: "Sonnet 4.6", default: false },
	];
	const SESSION = "6b1f0c2e-58a4-4d7e-9a35-0e8f2b7d41c9";
	const held = (over: Partial<Agent> = {}) =>
		sample("agent-release", { control: "held", session: "s-release", model: "opus", ...over });

	// The header's picker comes before the composer's, which has the same name.
	const openMenu = async (name: string) => {
		const [header] = await screen.findAllByRole("button", { name });
		if (header) fireEvent.click(header);
		return screen.findByRole("menu");
	};

	it("picks the model for an agent MaxHQ holds", async () => {
		const agent = held();
		const { source } = show(agent, undefined, MODELS);
		const setModel = vi.spyOn(source, "setModel");
		const menu = await openMenu("Model: Opus 4.8");
		expect(within(menu).getByText("Model, Claude Max")).toBeInTheDocument();
		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Sonnet 4\.6/ }));
		expect(setModel).toHaveBeenCalledWith("agent-release", "sonnet", undefined);
	});

	it("moves an agent MaxHQ holds to another vendor signed in on its device", async () => {
		const agent = held();
		const { source } = show(agent, undefined, MODELS);
		const setModel = vi.spyOn(source, "setModel");
		const menu = await openMenu("Model: Opus 4.8");
		const groups = within(menu)
			.getAllByText(/^Move to /)
			.map((label) => label.textContent);
		// devbox's logins aren't on the agent's device.
		expect(groups).toEqual(["Move to Codex Plus", "Move to Cursor Pro", "Move to Gemini"]);
		const codex = within(menu).getByRole("group", { name: "Move to Codex Plus" });
		fireEvent.click(within(codex).getByRole("menuitem", { name: "Default model" }));
		expect(setModel).toHaveBeenCalledWith("agent-release", "", CODEX);
	});

	it("says why the model didn't change, until you dismiss it", async () => {
		const agent = held();
		const { source } = show(agent, undefined, MODELS);
		vi.spyOn(source, "setModel").mockResolvedValueOnce({
			status: "error",
			error: { kind: "not_connected", device: "Mac" },
		});
		const menu = await openMenu("Model: Opus 4.8");
		fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Sonnet 4\.6/ }));
		const alert = await screen.findByText("Couldn't change the model. Mac isn't connected.");
		expect(alert).toHaveAttribute("role", "alert");
		fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
		expect(alert).not.toBeInTheDocument();
	});

	it("only names the model of a session in a terminal", async () => {
		show(sample("agent-release", { control: "observed", model: "opus" }), undefined, MODELS);
		expect(await screen.findByText("Opus 4.8")).toBeInTheDocument();
		expect(screen.queryByRole("button", { name: /^Model/ })).not.toBeInTheDocument();
	});

	it("stops the agent from the More menu", async () => {
		const agent = held();
		const { source } = show(agent);
		const stopAgent = vi.spyOn(source, "stopAgent");
		const menu = await openMenu("More");
		fireEvent.click(within(menu).getByRole("menuitem", { name: /Stop agent/ }));
		expect(stopAgent).toHaveBeenCalledWith("agent-release");
	});

	it("copies the session ID from the More menu, and says it did", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			show(sample("agent-release", { vendor_session_id: SESSION }));
			const menu = await openMenu("More");
			// Only MaxHQ's own agents can be stopped from here.
			expect(within(menu).queryByRole("menuitem", { name: /Stop agent/ })).toBeNull();
			const item = within(menu).getByRole("menuitem", { name: /Copy session ID/ });
			fireEvent.click(item);
			expect(writeText).toHaveBeenCalledWith(SESSION);
			await waitFor(() => expect(item).toHaveTextContent("Copied"));
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("shows the agent's branch, and copies it on a click", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			show(sample("agent-release", { branch: "fix/release-notes" }));
			const chip = await screen.findByRole("button", { name: "fix/release-notes" });
			expect(chip).toHaveAttribute("title", "Copy branch fix/release-notes");
			fireEvent.click(chip);
			expect(writeText).toHaveBeenCalledWith("fix/release-notes");
			await waitFor(() => expect(chip).toHaveTextContent("Copied"));
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("calls a Codex session's ID a thread ID", async () => {
		show(sample("agent-hdr", { vendor_session_id: "thread-1" }));
		const menu = await openMenu("More");
		expect(within(menu).getByRole("menuitem", { name: /Copy thread ID/ })).toBeInTheDocument();
	});

	it("offers to link a PR in More while the agent has none", async () => {
		show(sample("agent-usage-popover"));
		fireEvent.click(await screen.findByRole("button", { name: "More" }));
		expect(
			await screen.findByRole("menuitem", { name: /Link a pull request/ })
		).toBeInTheDocument();
	});

	it("copies the conversation as Markdown from More, and says it did", async () => {
		const writeText = vi.fn().mockResolvedValue(undefined);
		Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
		try {
			const agent = sample("agent-usage-popover", { pulls: [pullLink(142)] });
			show(agent);
			const menu = await openMenu("More");
			// With nothing else to offer, More still exports. Outside the app
			// there's no save dialog, so it copies.
			expect(within(menu).getAllByRole("menuitem")).toHaveLength(1);
			const item = within(menu).getByRole("menuitem", { name: /Copy as Markdown/ });
			fireEvent.click(item);
			await waitFor(() => expect(item).toHaveTextContent("Copied as Markdown"));
			const text = String(writeText.mock.calls[0]?.[0]);
			expect(text.startsWith(`# ${agent.title}\n\n## You`)).toBe(true);
		} finally {
			Reflect.deleteProperty(navigator, "clipboard");
		}
	});

	it("saves the conversation where you pick in the app, and names the file", async () => {
		const app = globalThis as { isTauri?: boolean };
		const saved: Record<string, unknown>[] = [];
		let answer: { name: string; folder: string } | null = null;
		app.isTauri = true;
		mockIPC((command, args) => {
			if (command !== "save_export") return null;
			saved.push(args as Record<string, unknown>);
			return answer;
		});
		try {
			const agent = sample("agent-usage-popover", { pulls: [pullLink(142)] });
			show(agent);
			const menu = await openMenu("More");
			const item = within(menu).getByRole("menuitem", { name: /Export as Markdown…/ });
			expect(
				within(menu).getByRole("menuitem", { name: /Copy as Markdown/ })
			).toBeInTheDocument();

			// Cancelling the dialog leaves the item as it was.
			fireEvent.click(item);
			await waitFor(() => expect(saved).toHaveLength(1));
			await waitFor(() => expect(item).toHaveTextContent("Export as Markdown…"));
			expect(String(saved[0]?.text).startsWith(`# ${agent.title}\n\n## You`)).toBe(true);
			expect(saved[0]?.name).toMatch(/^Usage meter popover \d{4}-\d{2}-\d{2}\.md$/);

			answer = { name: "Release notes.md", folder: "/Users/me/Notes" };
			fireEvent.click(item);
			await waitFor(() => expect(item).toHaveTextContent("Saved as Markdown"));
			expect(item).toHaveTextContent("Release notes.md");
		} finally {
			clearMocks();
			delete app.isTauri;
			localStorage.clear();
		}
	});
});

describe("outside session banner", () => {
	const SESSION = "6b1f0c2e-58a4-4d7e-9a35-0e8f2b7d41c9";

	it("keeps Continue off while the terminal runs the session, and says why", () => {
		const agent = agentOf({ id: "a1", vendor_session_id: SESSION });
		const { continueHere } = show(agent, <OutsideBanner agent={agent} />);
		expect(
			screen.getByRole("heading", { name: /Running in a terminal.*Read only here\./ })
		).toBeInTheDocument();
		const button = screen.getByRole("button", { name: "Continue in MaxHQ" });
		expect(button).toHaveAttribute("aria-disabled", "true");
		expect(button).toHaveAccessibleDescription(
			"Turns on when the session in the terminal ends. Quit it there with /exit. A session can only run in one place at a time."
		);
		fireEvent.click(button);
		expect(continueHere).not.toHaveBeenCalled();
		expect(screen.queryByRole("button", { name: /resume command/ })).not.toBeInTheDocument();
	});

	it("turns Continue on once the terminal session ends, and continues it here", async () => {
		const agent = agentOf({
			id: "a1",
			vendor_session_id: SESSION,
			link: { state: "down", reason: "exited", since: NOW - 2 * MIN },
		});
		const { continueHere } = show(agent, <OutsideBanner agent={agent} />);
		expect(
			screen.getByRole("heading", {
				name: "The terminal session ended. You can continue it here.",
			})
		).toBeInTheDocument();
		const button = screen.getByRole("button", { name: "Continue in MaxHQ" });
		expect(button).toBeEnabled();
		expect(button).not.toHaveAttribute("aria-disabled");
		fireEvent.click(button);
		expect(continueHere).toHaveBeenCalledWith("a1");
		await waitFor(() => expect(button).toBeDisabled());
	});

	it("shows why continuing failed and lets you try again", async () => {
		const agent = agentOf({
			id: "a1",
			link: { state: "down", reason: "exited", since: NOW - 2 * MIN },
		});
		const { continueHere } = show(agent, <OutsideBanner agent={agent} />);
		const refused: ActionResult = {
			status: "ok",
			data: { status: "refused", reason: "the session is still open in another terminal" },
		};
		continueHere.mockResolvedValueOnce(refused);
		fireEvent.click(screen.getByRole("button", { name: "Continue in MaxHQ" }));
		expect(await screen.findByRole("alert")).toHaveTextContent(
			"The session is still open in another terminal."
		);
		expect(screen.getByRole("button", { name: "Continue in MaxHQ" })).toBeEnabled();
	});

	it("says when it can't reach the device, and when the device last answered", async () => {
		const agent = sample("agent-sync-drift", { activity: { state: "not_accessible" } });
		render(
			<ServicesProvider
				source={mockSource({ now: () => NOW, tick: false, scenario: "tailscale_off" })}
			>
				<FixedNow value={NOW}>
					<OutsideBanner agent={agent} />
				</FixedNow>
			</ServicesProvider>
		);
		expect(
			await screen.findByRole("heading", { name: "Can't reach devbox right now." })
		).toBeInTheDocument();
		expect(
			screen.getByText(
				"It last answered at 14:44, 4 min ago. The session may still run in the terminal there. This view shows what MaxHQ saw last."
			)
		).toBeInTheDocument();
		expect(screen.queryByText(/follows along/)).not.toBeInTheDocument();
		expect(
			screen.getByRole("button", { name: "Continue in MaxHQ" })
		).toHaveAccessibleDescription(
			"Turns on once MaxHQ reaches devbox again and the session in the terminal has ended."
		);
	});

	it("gives the reason an agent stopped with an error", () => {
		const agent = sample("agent-egress");
		show(agent, <OutsideBanner agent={agent} />);
		expect(
			screen.getByRole("heading", {
				name: "The terminal session stopped with an error. You can continue it here.",
			})
		).toBeInTheDocument();
		expect(
			screen.getByText(
				"Exited with status 1. Claude keeps the history and context. The terminal is no longer attached."
			)
		).toBeInTheDocument();
	});

	it("offers the resume command for the vendor's CLI", () => {
		expect(resumeCommand(agentOf({ id: "a1", vendor_session_id: SESSION }))).toBe(
			`claude --resume ${SESSION}`
		);
		expect(resumeCommand(agentOf({ id: "a1", vendor: "codex", vendor_session_id: "s1" }))).toBe(
			"codex resume s1"
		);
		expect(resumeCommand(agentOf({ id: "a1" }))).toBeNull();
		const agent = agentOf({
			id: "a1",
			vendor_session_id: SESSION,
			link: { state: "down", reason: "exited", since: NOW },
		});
		show(agent, <OutsideBanner agent={agent} />);
		expect(screen.getByRole("button", { name: "Copy resume command" })).toHaveAttribute(
			"title",
			`claude --resume ${SESSION}`
		);
	});
});

describe("take over from the terminal", () => {
	// A session in a terminal on this Mac, whose maxd knows how to take over.
	const showNewer = (over: Partial<Agent> = {}) => {
		const agent = agentOf({ id: "a1", workspace: "ws-mac-maxhq", ...over });
		const older = sourceWith(agent);
		const snapshot = older.snapshot;
		const newer: DataSource["snapshot"] = async () => {
			const state = await snapshot();
			return {
				...state,
				entities: state.entities.map((e) =>
					e.entity === "device_link" ? { ...e, protocol: TAKE_OVER_SINCE } : e
				),
			};
		};
		const source = Object.assign(older, { snapshot: newer });
		const takeOver = vi.spyOn(source, "takeOver");
		render(
			<ServicesProvider source={source}>
				<FixedNow value={NOW}>
					<OutsideBanner agent={agent} />
					<ConfirmHost />
				</FixedNow>
			</ServicesProvider>
		);
		return { takeOver };
	};

	it("is not offered by an older maxd, which keeps today's banner", () => {
		const agent = agentOf({ id: "a1", workspace: "ws-mac-maxhq" });
		show(agent, <OutsideBanner agent={agent} />);
		expect(screen.queryByRole("button", { name: "Take over" })).not.toBeInTheDocument();
		expect(screen.getByRole("button", { name: "Continue in MaxHQ" })).toHaveAttribute(
			"aria-disabled",
			"true"
		);
	});

	it("asks first while idle, names the risk, then stops the terminal at once", async () => {
		const { takeOver } = showNewer();
		fireEvent.click(await screen.findByRole("button", { name: "Take over" }));
		expect(screen.getByText(/Or quit it in the terminal yourself with/)).toBeInTheDocument();
		const dialog = await screen.findByRole("alertdialog", { name: "Take over this session?" });
		expect(dialog).toHaveTextContent(
			"This stops claude in the terminal on this Mac. The terminal window stays open, but the session leaves it and continues here."
		);
		expect(within(dialog).queryByRole("button", { name: "Stop now" })).not.toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("button", { name: "Take over" }));
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("a1", "now"));
	});

	it("waits for the turn, or stops now, while a turn runs", async () => {
		const { takeOver } = showNewer({ activity: { state: "busy" } });
		fireEvent.click(await screen.findByRole("button", { name: "Take over" }));
		const dialog = await screen.findByRole("alertdialog");
		expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeInTheDocument();
		fireEvent.click(within(dialog).getByRole("button", { name: "Take over after this turn" }));
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("a1", "after_turn"));

		fireEvent.click(screen.getByRole("button", { name: "Take over" }));
		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Stop now" })
		);
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("a1", "now"));
	});

	it("sends nothing when you cancel the question", async () => {
		const { takeOver } = showNewer();
		fireEvent.click(await screen.findByRole("button", { name: "Take over" }));
		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" })
		);
		await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
		expect(takeOver).not.toHaveBeenCalled();
	});

	it("says it waits for the turn, and lets you call it off", async () => {
		const { takeOver } = showNewer({ activity: { state: "busy" }, taking_over: true });
		expect(await screen.findByRole("status")).toHaveTextContent(
			"MaxHQ stops claude in the terminal on this Mac once this turn ends, then continues the session here."
		);
		expect(screen.queryByRole("button", { name: "Take over" })).not.toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
		expect(takeOver).toHaveBeenCalledWith("a1", "cancel");
	});

	it("is not offered for Cursor", async () => {
		showNewer({ vendor: "cursor" });
		expect(await screen.findByRole("heading", { name: /Running in a terminal/ })).toBeVisible();
		expect(screen.queryByRole("button", { name: "Take over" })).not.toBeInTheDocument();
	});
});

describe("take over from a sub-agent's view", () => {
	// A Codex session in a terminal on this Mac with a sub-agent, and more
	// agents under it when given, as a maxd that speaks `protocol` sends them.
	const showSub = (
		lead: Partial<Agent> = {},
		protocol: number = TAKE_OVER_SINCE,
		more: readonly Agent[] = []
	) => {
		const top = agentOf({ id: "lead", workspace: "ws-mac-maxhq", vendor: "codex", ...lead });
		const sub = agentOf({
			id: "sub",
			workspace: "ws-mac-maxhq",
			vendor: top.vendor,
			parent: "lead",
			pid: null,
		});
		const source = mockSource({ now: () => NOW, tick: false });
		const snapshot = source.snapshot;
		const withSession: DataSource["snapshot"] = async () => {
			const state = await snapshot();
			return {
				...state,
				entities: [
					...state.entities.map((e) =>
						e.entity === "device_link" ? { ...e, protocol } : e
					),
					...[top, sub, ...more].map((a) => ({ entity: "agent" as const, ...a })),
				],
			};
		};
		Object.assign(source, { snapshot: withSession });
		const takeOver = vi.spyOn(source, "takeOver");
		const ui = createUiStore();
		render(
			<ServicesProvider source={source} ui={ui}>
				<FixedNow value={NOW}>
					<OutsideBanner agent={more.at(-1) ?? sub} />
					<ConfirmHost />
				</FixedNow>
			</ServicesProvider>
		);
		return { takeOver, ui };
	};
	// The Take over button once the view holds the lead. Until then it
	// says MaxHQ can't see the session.
	const offered = () =>
		waitFor(() => {
			const button = screen.getByRole("button", { name: "Take over" });
			expect(button).not.toHaveAttribute("aria-disabled");
			return button;
		});

	it("takes over the Codex session it runs in, after saying what happens", async () => {
		const { takeOver } = showSub();
		fireEvent.click(await offered());
		expect(screen.queryByText(/Or quit it in the terminal/)).not.toBeInTheDocument();
		const dialog = await screen.findByRole("alertdialog", {
			name: "Take over this sub-agent's session?",
		});
		expect(dialog).toHaveTextContent(
			"This sub-agent runs inside a Codex session in the terminal on this Mac. Taking over stops codex there and continues that session here. Sub-agents still working stop with it. Codex keeps their history, so the session can pick them up again once it runs here."
		);
		fireEvent.click(within(dialog).getByRole("button", { name: "Take over" }));
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("lead", "now"));
	});

	it("waits for the lead's turn, not the sub-agent's", async () => {
		const { takeOver } = showSub({ activity: { state: "busy_in_subagents" } });
		fireEvent.click(await offered());
		fireEvent.click(
			within(await screen.findByRole("alertdialog")).getByRole("button", {
				name: "Take over after this turn",
			})
		);
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("lead", "after_turn"));
	});

	it("takes over the top lead of a Claude sub-agent's sub-agent", async () => {
		const deep = agentOf({ id: "deep", workspace: "ws-mac-maxhq", parent: "sub", pid: null });
		const { takeOver } = showSub({ vendor: "claude" }, TAKE_OVER_SINCE, [deep]);
		fireEvent.click(await offered());
		const dialog = await screen.findByRole("alertdialog");
		expect(dialog).toHaveTextContent(/inside a Claude session .* stops claude there/);
		fireEvent.click(within(dialog).getByRole("button", { name: "Take over" }));
		await waitFor(() => expect(takeOver).toHaveBeenCalledWith("lead", "now"));
	});

	it("says why it can't, without asking to exit the CLI", async () => {
		showSub({}, TAKE_OVER_SINCE - 1);
		const off = screen.getByRole("button", { name: "Take over" });
		await waitFor(() =>
			expect(off).toHaveAccessibleDescription("Taking over needs a newer maxd on this Mac.")
		);
		expect(off).toHaveAttribute("aria-disabled", "true");
		expect(screen.queryByText(/exit/i)).not.toBeInTheDocument();
	});

	it("says it waits for the session's turn, and lets you call it off", async () => {
		const { takeOver } = showSub({ activity: { state: "busy" }, taking_over: true });
		expect(await screen.findByRole("status")).toHaveTextContent(
			"MaxHQ stops codex in the terminal on this Mac once the session's turn ends, then continues the session here."
		);
		fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
		expect(takeOver).toHaveBeenCalledWith("lead", "cancel");
	});

	it("points to the session once it continues in MaxHQ", async () => {
		const { ui } = showSub({ control: "held" });
		const moved = await screen.findByRole("region", { name: "Session continues in MaxHQ" });
		expect(moved).toHaveTextContent("This sub-agent's session continues in MaxHQ.");
		expect(screen.queryByRole("button", { name: "Take over" })).not.toBeInTheDocument();
		fireEvent.click(within(moved).getByRole("button", { name: "Open session" }));
		expect(ui.getState().selectedAgent).toBe("lead");
	});
});

describe("resume banner", () => {
	const stopped = (over: Partial<Agent> = {}) =>
		agentOf({
			id: "a1",
			control: "held",
			pid: null,
			activity: { state: "paused", reason: "by_you" },
			link: { state: "down", reason: "stopped", since: NOW - 2 * MIN },
			...over,
		});

	it("shows over the composer of an agent you stopped, and resumes it", async () => {
		const agent = sample("agent-stop-times", { control: "held" });
		const { continueHere } = show(agent);
		const banner = await screen.findByRole("region", { name: "Agent stopped" });
		expect(within(banner).getByRole("heading", { name: "Stopped" })).toBeInTheDocument();
		const resume = within(banner).getByRole("button", { name: "Resume" });
		fireEvent.click(resume);
		expect(continueHere).toHaveBeenCalledWith("agent-stop-times");
		await waitFor(() => expect(resume).toBeDisabled());
	});

	it("says why resuming failed and lets you try again", async () => {
		const agent = stopped();
		const { continueHere } = show(agent, <ResumeBanner agent={agent} />);
		continueHere.mockResolvedValueOnce({
			status: "ok",
			data: { status: "refused", reason: "the agent is still stopping" },
		});
		fireEvent.click(screen.getByRole("button", { name: "Resume" }));
		expect(await screen.findByRole("alert")).toHaveTextContent("The agent is still stopping.");
		expect(screen.getByRole("button", { name: "Resume" })).toBeEnabled();
	});

	it("says when it stopped for making no progress", () => {
		const agent = stopped({
			activity: { state: "paused", reason: "resumes_made_no_progress" },
		});
		show(agent, <ResumeBanner agent={agent} />);
		expect(screen.getByRole("heading", { name: "Paused after 3 resumes" })).toBeInTheDocument();
	});

	it("says when its CLI exited with an error", () => {
		const agent = stopped({ activity: { state: "error", reason: "exited with status 1" } });
		show(agent, <ResumeBanner agent={agent} />);
		expect(screen.getByRole("heading", { name: "Stopped with an error" })).toBeInTheDocument();
		expect(
			screen.getByText(/^Exited with status 1\. The conversation stays/)
		).toBeInTheDocument();
	});

	it("warns that a local ACP agent without session loading starts again without its history", async () => {
		// The sample finds a new agent with no session loading.
		const source = sourceWith(null);
		await source.setAcpAgent(
			"dev-mac",
			{ id: "acp-goose", device: "dev-mac", name: "Goose", program: "goose" },
			[]
		);
		const agent = stopped({ vendor: "acp", account: "acct-acp-goose" });
		render(
			<ServicesProvider source={source}>
				<ResumeBanner agent={agent} />
			</ServicesProvider>
		);
		expect(
			await screen.findByText(
				/Goose can't load an earlier session, so it starts again without this conversation's history\.$/
			)
		).toBeInTheDocument();
	});

	it("shows only for a stopped lead that MaxHQ holds", () => {
		expect(canResume(stopped())).toBe(true);
		expect(canResume(stopped({ control: "observed" }))).toBe(false);
		expect(canResume(stopped({ parent: "lead" }))).toBe(false);
		expect(canResume(stopped({ activity: { state: "idle" }, link: { state: "live" } }))).toBe(
			false
		);
		const error = { state: "error", reason: "exited with status 1" } as const;
		expect(canResume(stopped({ activity: error }))).toBe(true);
		expect(canResume(stopped({ activity: error, vendor: "codex" }))).toBe(false);
		expect(canResume(stopped({ activity: error, link: { state: "live" } }))).toBe(false);
	});
});

describe("sign-in card", () => {
	const signIn = () =>
		sample("agent-stop-times", {
			control: "held",
			vendor: "acp",
			account: GEMINI_ACCOUNT,
			activity: { state: "error", reason: "Gemini needs a sign-in." },
			link: { state: "down", reason: "exited", since: NOW - MIN },
		});

	it("lists the ways the agent signs in, and copies its command", async () => {
		show(signIn());
		const card = await screen.findByRole("region", { name: "Agent needs a sign-in" });
		expect(
			within(card).getByRole("heading", { name: "Gemini needs a sign-in" })
		).toBeInTheDocument();
		expect(card).toHaveTextContent("Sign in from a terminal on Mac");
		expect(card).toHaveTextContent("Log in with Google");
		expect(card).toHaveTextContent(
			"Use Gemini API key: Requires setting the GEMINI_API_KEY environment variable"
		);
		expect(within(card).getByRole("button", { name: "Copy command" })).toHaveAttribute(
			"title",
			"gemini --experimental-acp"
		);
	});

	it("starts the agent again on Try again", async () => {
		const { continueHere } = show(signIn());
		const card = await screen.findByRole("region", { name: "Agent needs a sign-in" });
		fireEvent.click(within(card).getByRole("button", { name: "Try again" }));
		expect(continueHere).toHaveBeenCalledWith("agent-stop-times");
	});

	it("isn't there for an agent that stopped for another reason", async () => {
		show(
			sample("agent-stop-times", {
				control: "held",
				vendor: "acp",
				account: GEMINI_ACCOUNT,
				activity: { state: "error", reason: "exited with status 1" },
				link: { state: "down", reason: "exited", since: NOW - MIN },
			})
		);
		await screen.findByRole("textbox", { name: /^Message/ });
		expect(
			screen.queryByRole("region", { name: "Agent needs a sign-in" })
		).not.toBeInTheDocument();
	});
});
