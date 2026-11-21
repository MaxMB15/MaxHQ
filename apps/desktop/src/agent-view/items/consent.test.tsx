// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type {
	AttentionAnswer,
	AttentionId,
	ItemBody,
	ItemStatus,
	Question,
} from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { type RowContext, RowProvider } from "@maxhq/shared/agent-view/context";
import { clearKept } from "@maxhq/shared/agent-view/kept";
import { setLanguageChoice } from "@maxhq/shared/i18n";
import { NOW } from "@maxhq/shared/test/build";
import { timelineItem } from "@maxhq/shared/timeline/build";
import { createTimelineStore } from "@maxhq/shared/timeline/store";

import { ConsentFacts } from "@/components/consent-facts";

import { QuestionCard } from "./question";
import type { ItemOf } from "./types";

afterEach(() => {
	cleanup();
	clearKept();
});

const QUESTION = "container-reach";
const REMEMBER = "container-reach-remember";
const REMEMBER_LABEL = "Remember for every call outside the container";
const WHERE =
	"Kept on the permission, so it answers every later call that reaches outside the container, to any agent, device or test machine. To change it later, open the menu in this agent's header, pick Change what it can run, and set Containers.";

const REACH: Question = {
	id: QUESTION,
	header: "Reach outside the container",
	text:
		"Fixer runs in the container web and asks to send a message to Lead on the host.\n\n" +
		"That agent runs outside the container, without its network limits or file walls.\n\n" +
		"Agent: Fixer\nContainer: web\nCall: send_agent",
	options: [
		{ label: "Allow", description: "Lets this call go, this once." },
		{ label: "Deny", description: "Refuses this call." },
	],
	multi: false,
	other: false,
	secret: false,
	consent: {
		title: "Reach outside the container",
		risk:
			"Fixer runs in the container web and asks to send a message to Lead on the host. " +
			"That agent runs outside the container, without its network limits or file walls.",
		changes: [],
		facts: [
			{ label: "Agent", value: "Fixer" },
			{ label: "Container", value: "web" },
			{ label: "Call", value: "send_agent" },
		],
		rows: [],
		remember: { id: REMEMBER, label: REMEMBER_LABEL, settings: WHERE },
	},
};

const HOOKS_WHERE =
	"Agents in Accept edits or Auto may then change your git hooks in this project without asking. Turn it off in Project settings, under Permissions.";

/** The card maxd shows before an agent changes a git hook. */
const HOOKS: Question = {
	id: "edit-kind",
	header: "Git hooks",
	text: "Cursor asks to change .husky/pre-commit. Git hooks run on your next commit, with your keys.",
	options: [
		{ label: "Allow", description: null },
		{ label: "Deny", description: null },
	],
	multi: false,
	other: false,
	secret: false,
	consent: {
		title: "Change git hooks?",
		risk: "Git hooks run on your next commit, with your keys.",
		changes: [],
		facts: [{ label: "File", value: ".husky/pre-commit" }],
		rows: [],
		remember: {
			id: "edit-kind-remember",
			label: "Remember for this project",
			settings: HOOKS_WHERE,
		},
		edit: "git_hooks",
	},
};

const show = (
	status: ItemStatus = "waiting",
	row: Partial<RowContext> = {},
	question: Question = REACH
) => {
	const answer = vi.fn<
		(attention: AttentionId, answer: AttentionAnswer) => Promise<ActionResult>
	>(async () => ({ status: "ok", data: { status: "done" } }));
	const body: ItemBody = {
		kind: "question",
		attention: "att-reach",
		questions: [question],
		answers: null,
	};
	const item = timelineItem(body, {
		id: "item-reach",
		status,
		started_at: NOW - 4000,
		completed_at: status === "waiting" ? null : NOW,
	}) as ItemOf<"question">;
	render(
		<FixedNow value={NOW}>
			<RowProvider
				value={{
					agent: "a1",
					vendor: "claude",
					timeline: createTimelineStore(),
					readOnly: false,
					remote: false,
					answer,
					subAgentsShown: false,
					toggleSubAgents: () => {},
					...row,
				}}
			>
				<QuestionCard item={item} />
			</RowProvider>
		</FixedNow>
	);
	return answer;
};

/** What a pick sends, with the box's own answer when it's ticked. */
const sent = (label: string, remember = false) => {
	const answers = [{ id: QUESTION, values: [label] }];
	if (remember) answers.push({ id: REMEMBER, values: [REMEMBER_LABEL] });
	return { type: "questions", answers };
};

describe("consent card", () => {
	it("shows what's asked, the risk and the details", () => {
		show();
		expect(screen.getByRole("region", { name: "Consent request" })).toBeInTheDocument();
		expect(screen.getByText("Reach outside the container")).toBeInTheDocument();
		expect(screen.getByText(/without its network limits or file walls/)).toBeInTheDocument();
		expect(screen.getByText("Container")).toBeInTheDocument();
		expect(screen.getByText("web")).toBeInTheDocument();
		expect(screen.getByText("send_agent")).toBeInTheDocument();
		expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual([
			"Allow",
			"Deny",
		]);
	});

	it("answers for this call alone without Remember", () => {
		const answer = show();
		expect(screen.queryByText(WHERE)).toBeNull();
		fireEvent.click(screen.getByRole("button", { name: "Allow" }));
		expect(answer).toHaveBeenCalledWith("att-reach", sent("Allow"));
	});

	it("refuses this call alone without Remember", () => {
		const answer = show();
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		expect(answer).toHaveBeenCalledWith("att-reach", sent("Deny"));
	});

	it("keeps the answer with Remember, and says where to change it", () => {
		const answer = show();
		fireEvent.click(screen.getByRole("checkbox", { name: REMEMBER_LABEL }));
		expect(screen.getByText(WHERE)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		expect(answer).toHaveBeenCalledWith("att-reach", sent("Deny", true));
	});

	it("keeps an Allow with Remember too", () => {
		const answer = show();
		fireEvent.click(screen.getByRole("checkbox", { name: REMEMBER_LABEL }));
		fireEvent.click(screen.getByRole("button", { name: "Allow" }));
		expect(answer).toHaveBeenCalledWith("att-reach", sent("Allow", true));
	});

	it("takes an answer in a session MaxHQ only watches, since MaxHQ asks it", () => {
		const answer = show("waiting", { readOnly: true });
		fireEvent.click(screen.getByRole("button", { name: "Deny" }));
		expect(answer).toHaveBeenCalledWith("att-reach", sent("Deny"));
	});

	it("names a kind of file, what it runs and where to turn the box off", () => {
		const answer = show("waiting", {}, HOOKS);
		expect(screen.getByText("Change git hooks?")).toBeInTheDocument();
		expect(
			screen.getByText("Git hooks run on your next commit, with your keys.")
		).toBeInTheDocument();
		expect(screen.getByText(".husky/pre-commit")).toBeInTheDocument();
		fireEvent.click(screen.getByRole("checkbox", { name: "Remember for this project" }));
		expect(screen.getByText(HOOKS_WHERE)).toBeInTheDocument();
		fireEvent.click(screen.getByRole("button", { name: "Allow" }));
		expect(answer).toHaveBeenCalledWith("att-reach", {
			type: "questions",
			answers: [
				{ id: "edit-kind", values: ["Allow"] },
				{ id: "edit-kind-remember", values: ["Remember for this project"] },
			],
		});
	});

	it("words a kind of file in your language and sends maxd's labels", () => {
		setLanguageChoice("de");
		try {
			const answer = show("waiting", {}, HOOKS);
			expect(screen.queryByText("Change git hooks?")).toBeNull();
			fireEvent.click(screen.getByRole("button", { name: "Ablehnen" }));
			expect(answer).toHaveBeenCalledWith("att-reach", {
				type: "questions",
				answers: [{ id: "edit-kind", values: ["Deny"] }],
			});
		} finally {
			setLanguageChoice("en");
		}
	});

	it("closes under its title when nobody answered", () => {
		show("cancelled");
		expect(screen.getByText("Reach outside the container")).toBeInTheDocument();
		expect(screen.getByText("Closed without an answer")).toBeInTheDocument();
	});
});

describe("consent facts", () => {
	it("keep a long value whole, with its lines, and say when there's more below", () => {
		// The value's text runs taller than its box shows.
		const spies = [
			vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(900),
			vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(220),
		];
		const prompt = `Check the build.\n${"And again. ".repeat(80)}\nThen delete it.`;
		render(<ConsentFacts facts={[{ label: "Prompt", value: prompt }]} />);
		expect(screen.getByText(/^Check the build\./).textContent).toBe(prompt);
		expect(screen.getByText("More below. Scroll to read it all.")).toBeInTheDocument();
		spies.forEach((spy) => spy.mockRestore());
	});
});
