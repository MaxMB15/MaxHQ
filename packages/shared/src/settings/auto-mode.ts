// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Auto mode approves what looks safe and asks about the rest. Among what
// it approves are the repo's own test, build and lint scripts, which can
// run any code, so the first time you pick Auto the app says so and asks
// you to confirm. An agent can be in Auto without your pick, as a device's
// default puts it there, so the first start or message in Auto asks too,
// and offers Ask first instead. The answer stays with the app's install.

import { t } from "../i18n";

import type { Agent, AgentId, DeviceId, PermissionMode } from "../bindings";
import { attempt, failureOf } from "../composer/send";
import type { DataSource } from "../data/source";
import type { ViewState } from "../view/replica";
import { deviceName } from "../view/select";
import { deviceSettingsOf } from "./device-settings";
import { MODE_LABEL } from "./permission";

/**
 * What Auto lets an agent do without asking, as a picker's line under it.
 * It's the English key, so `t` it where it shows.
 */
export const AUTO_RUNS =
	"Runs the repo's own test, build and lint scripts without asking. Use it on repos you trust";

/** The same, as a sentence about Auto where its line doesn't fit. `t` it too. */
export const AUTO_NOTE =
	"Auto runs the repo's own test, build and lint scripts without asking. Use it on repos you trust.";

/** The question the app asks the first time you pick Auto. */
export const AUTO_QUESTION = {
	get title() {
		return t("Use Auto mode?");
	},
	get detail() {
		return t(
			"Auto approves what looks safe and asks about the rest. It runs the repo's own test, build and lint scripts without asking, and those can run any code. Use it on repos you trust."
		);
	},
	get action() {
		return t("Use Auto");
	},
};

/**
 * The question the app asks the first time an agent would start or take a
 * message in Auto that you didn't pick. Its other answer is Ask first.
 */
export const AUTO_DEFAULT_QUESTION = {
	get title() {
		return AUTO_QUESTION.title;
	},
	get detail() {
		return t(
			"This agent is set to Auto. {{detail}} {{ask}} instead switches it and its device's default to {{ask}}.",
			{ detail: AUTO_QUESTION.detail, ask: MODE_LABEL.ask }
		);
	},
	get action() {
		return AUTO_QUESTION.action;
	},
	get other() {
		return t("{{ask}} instead", { ask: MODE_LABEL.ask });
	},
};

/** Where an app keeps whether you confirmed Auto. Either call may throw. */
export type ConsentStore = {
	readonly read: () => string | null;
	readonly write: (value: string) => void;
};

/** Whether you confirmed Auto on this install, and a way to remember it. */
export type AutoConsent = {
	readonly confirmed: () => boolean;
	readonly remember: () => void;
};

const CONFIRMED = "confirmed";

/**
 * The consent kept in `store`. A store that can't be read counts as not
 * confirmed, and one that can't be written asks again next time.
 */
export const keptConsent = (store: ConsentStore): AutoConsent => ({
	confirmed: () => {
		try {
			return store.read()?.trim() === CONFIRMED;
		} catch {
			return false;
		}
	},
	remember: () => {
		try {
			store.write(CONFIRMED);
		} catch (error) {
			console.warn(t("Couldn't keep the Auto mode answer"), error);
		}
	},
});

/** Whether running in `mode` waits for you to confirm Auto. */
export const needsAutoConsent = (mode: PermissionMode, consent: AutoConsent): boolean =>
	mode === "auto" && !consent.confirmed();

/**
 * Runs `go` once you may switch to `mode`: right away for any mode but
 * Auto, and for Auto once you confirm it. `ask` puts the question, the
 * first time only, and calls `yes` on a yes, which is remembered.
 */
export const whenModeAllowed = (
	mode: PermissionMode,
	consent: AutoConsent,
	ask: (yes: () => void) => void,
	go: () => void
): void => {
	if (!needsAutoConsent(mode, consent)) {
		go();
		return;
	}
	ask(() => {
		consent.remember();
		go();
	});
};

/** Your answer to `AUTO_DEFAULT_QUESTION`, as the mode it runs in. */
export type AutoChoice = Extract<PermissionMode, "auto" | "ask">;

/** Where Ask first instead takes effect. */
export type AskFirstPlace = {
	readonly source: Pick<DataSource, "setPermission" | "setDeviceSettings">;
	readonly view: ViewState;
	/** The device the agent runs on, whose default changes. */
	readonly device: DeviceId;
	/** The agent, or null for one that hasn't started yet. */
	readonly agent: AgentId | null;
};

/**
 * What the answer settled: the mode to start or send in, or null to do
 * nothing, and why a change didn't go through, if one didn't.
 */
export type AgreedMode = {
	readonly mode: PermissionMode | null;
	readonly failure: string | null;
};

/**
 * The mode to start an agent in or send it a message, for an agent set to
 * `mode` without your pick. Auto waits for your answer until you say Use
 * Auto once, which is remembered. Ask first instead switches the agent,
 * once it runs, and the device's default to Ask first; an agent that
 * can't switch gets nothing. A question you dismiss leaves it all as it was.
 */
export const agreeOnMode = async (
	mode: PermissionMode,
	consent: AutoConsent,
	ask: () => Promise<AutoChoice | null>,
	place: AskFirstPlace
): Promise<AgreedMode> => {
	if (!needsAutoConsent(mode, consent)) return { mode, failure: null };
	const choice = await ask();
	if (choice === "auto") consent.remember();
	if (choice !== "ask") return { mode: choice, failure: null };
	return askFirstInstead(place);
};

const askFirstInstead = async ({
	source,
	view,
	device,
	agent,
}: AskFirstPlace): Promise<AgreedMode> => {
	const settings = {
		device,
		...deviceSettingsOf(view, device),
		default_permission: "ask" as const,
	};
	const [switched, defaulted] = await Promise.all([
		agent === null ? null : attempt(() => source.setPermission(agent, "ask")),
		attempt(() => source.setDeviceSettings(settings)),
	]);
	const stuck = switched && failureOf(switched, "permission");
	if (stuck) return { mode: null, failure: stuck };
	const why = failureOf(defaulted);
	if (why === null) return { mode: "ask", failure: null };
	const failed = t("Couldn't make {{mode}} the default on {{device}}.", {
		mode: MODE_LABEL.ask,
		device: deviceName(view, device),
	});
	return { mode: "ask", failure: why ? `${failed} ${why}.` : failed };
};

/** Whether a message goes, and why a change didn't go through, if one didn't. */
export type SendGate = {
	readonly go: boolean;
	readonly failure: string | null;
};

/**
 * Whether a message to `agent` goes, as `agreeOnMode` settles the mode it
 * runs in. An agent without a mode, or one the view can't place, sends.
 */
export const agreeToSend = async (
	agent: Pick<Agent, "id" | "workspace" | "permission">,
	consent: AutoConsent,
	ask: () => Promise<AutoChoice | null>,
	source: AskFirstPlace["source"],
	view: ViewState
): Promise<SendGate> => {
	const device = view.tables.workspace.get(agent.workspace)?.device;
	if (!agent.permission || !device) return { go: true, failure: null };
	const running = agent.permission.effective ?? agent.permission.requested;
	const place = { source, view, device, agent: agent.id };
	const { mode, failure } = await agreeOnMode(running, consent, ask, place);
	return { go: mode !== null, failure };
};
