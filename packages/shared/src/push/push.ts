// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone does with pushes, apart from the calls that reach iOS:
// which kinds you want, what a push points at, and how a lock screen
// button turns into an answer. A push comes as maxd writes it, with the
// agent as its thread, the request under "e", and what the extension
// opened under "m". An iPhone's pushes go through APNs and an Android
// phone's through FCM, which only a maxd from FCM_SINCE on knows.

import { formatList, t } from "../i18n";

import {
	type AgentId,
	type AttentionAnswer,
	type AttentionId,
	type AttentionOption,
	type DeviceId,
	FCM_SINCE,
	type PushEnvironment,
	type PushKind,
	type PushService,
	type PushToken,
	type Timestamp,
} from "../bindings";
import { approvalChoices } from "../agent-view/answer";
import type { ActionResult } from "../data/source";
import { agentSynced } from "../status/reach";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** The switches, in the order settings shows them, with what each is for. */
export const PUSH_KINDS: readonly {
	readonly kind: PushKind;
	readonly label: string;
	readonly detail: string;
}[] = [
	{
		kind: "needs_input",
		get label() {
			return t("Needs input");
		},
		get detail() {
			return t(
				"A permission, a question or a secret. Open it to answer. Deny also works from the lock screen."
			);
		},
	},
	{
		kind: "error",
		get label() {
			return t("Error");
		},
		get detail() {
			return t("An agent stopped on an error");
		},
	},
	{
		kind: "finished",
		get label() {
			return t("Finished");
		},
		get detail() {
			return t("A turn ended while you were away");
		},
	},
	{
		kind: "limit",
		get label() {
			return t("Limit");
		},
		get detail() {
			return t("An agent waits for a usage limit to reset");
		},
	},
];

/** Every kind is on until you turn one off, as on the Mac. */
export const DEFAULT_KINDS: readonly PushKind[] = PUSH_KINDS.map((each) => each.kind);

/** The kinds with one switched, in settings order. */
export const switchKind = (kinds: readonly PushKind[], kind: PushKind, on: boolean): PushKind[] =>
	DEFAULT_KINDS.filter((each) => (each === kind ? on : kinds.includes(each)));

/** The kinds kept as JSON, or every kind when there are none or they don't read. */
export const readKinds = (text: string | null): PushKind[] => {
	if (text === null) return [...DEFAULT_KINDS];
	try {
		const kept: unknown = JSON.parse(text);
		if (!Array.isArray(kept)) return [...DEFAULT_KINDS];
		return DEFAULT_KINDS.filter((kind) => kept.includes(kind));
	} catch {
		return [...DEFAULT_KINDS];
	}
};

/** The APNs host a build's tokens belong to: a development build's go to the sandbox. */
export const pushEnvironment = (variant: unknown): PushEnvironment =>
	variant === "development" ? "sandbox" : "production";

/**
 * What the phone gives the devices. It goes again when the token or the
 * kinds change, and the core gives it to each device as its link comes up.
 * `service` is the one the token belongs to: FCM for an Android phone,
 * where `topic` is the app's package name and `environment` goes unused.
 */
export const pushToken = (
	token: string,
	topic: string,
	environment: PushEnvironment,
	kinds: readonly PushKind[],
	service: PushService = "apns"
): PushToken => ({ token, topic, environment, kinds: [...kinds], service });

/**
 * Whether `device`'s maxd takes a phone's token for `service`. One from
 * before FCM_SINCE would refuse an Android phone's, so the core doesn't
 * give it one. A device whose protocol isn't known yet counts as taking
 * it, since the core gives it the token then.
 */
export const takesPushes = (view: ViewState, device: DeviceId, service: PushService): boolean => {
	if (service !== "fcm") return true;
	const protocol = linkForDevice(view, device)?.protocol ?? null;
	return protocol === null || protocol >= FCM_SINCE;
};

/** The devices this phone's pushes through `service` wait on an update of, by name. */
export const awaitingPushUpdate = (view: ViewState, service: PushService): string[] =>
	[
		...new Set(
			all(view, "device_link").flatMap((link) =>
				link.claim || link.device === null ? [] : [link.device]
			)
		),
	]
		.filter((device) => !takesPushes(view, device, service))
		.map((device) => deviceName(view, device))
		.sort((a, b) => a.localeCompare(b));

/**
 * What the phone says about the devices whose maxd is too old for its
 * pushes through `service`, or null when every device takes them.
 */
export const pushUpdateNote = (view: ViewState, service: PushService): string | null => {
	const names = awaitingPushUpdate(view, service);
	return names.length === 0
		? null
		: t("Update maxd on {{devices}} to get their pushes on this phone.", {
				devices: formatList(names),
				count: names.length,
			});
};

/** What a push points at. */
export type PushTarget = {
	readonly agent: AgentId;
	/** The request it's about, for one that waits on you. */
	readonly attention: AttentionId | null;
	/** The request's choices, when the extension opened them. */
	readonly options: readonly AttentionOption[];
};

const text = (value: unknown): string | null =>
	typeof value === "string" && value !== "" ? value : null;

const record = (value: unknown): Record<string, unknown> =>
	typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};

const optionList = (value: unknown): AttentionOption[] =>
	Array.isArray(value)
		? value.flatMap((each) => {
				const { id, label, kind } = record(each);
				return typeof id === "string" && typeof label === "string"
					? [{ id, label, ...(typeof kind === "string" ? { kind } : {}) }]
					: [];
			})
		: [];

/**
 * What a push's payload points at, or null for one that isn't MaxHQ's.
 * What the extension opened comes first, then what maxd sent in the clear.
 */
export const pushTarget = (payload: unknown): PushTarget | null => {
	const push = record(payload);
	const opened = record(push.m);
	const agent = text(opened.agent) ?? text(record(push.aps)["thread-id"]);
	if (!agent) return null;
	return {
		agent,
		attention: text(opened.attention) ?? text(push.e),
		options: optionList(opened.options),
	};
};

/** The payload of the note that stands in for a button whose answer didn't go. */
export const targetPayload = (target: PushTarget): Record<string, unknown> => ({
	m: { agent: target.agent, attention: target.attention },
});

/** The category a permission push sits in, as maxd names it. */
export const PERMISSION_CATEGORY = "permission";

/**
 * The one button on a permission push. A notification shows only part of a
 * request, so saying yes takes opening its card.
 */
export const DENY_ACTION = "deny";

/** The no among a request's choices, or null when none fits. */
export const denyAnswer = (options: readonly AttentionOption[]): AttentionAnswer | null => {
	const { deny } = approvalChoices(options);
	return deny ? { type: "option", id: deny.id } : null;
};

/** How long a button's answer keeps trying while the links come up. */
export const ANSWER_PATIENCE_MS = 20_000;

const RETRY_MS = 1_000;

/** Whether a failure may clear once the device's link is up and its view in. */
const mayClear = (result: ActionResult): boolean =>
	result.status === "error" &&
	(result.error.kind === "not_connected" || result.error.kind === "not_found");

/** How a button's answer went. */
export type ButtonOutcome =
	/** It went, or another answer was first, which the card shows. */
	| "sent"
	/** It never reached the device, so the phone says to open MaxHQ. */
	| "unreached";

/**
 * Sends a button's answer, trying again while the device isn't connected
 * or its view isn't in yet, as after the app opens from the lock screen.
 * The first answer wins on the device, so another one coming first is no
 * failure here.
 */
export const answerFromButton = async (
	send: () => Promise<ActionResult>,
	patience: number = ANSWER_PATIENCE_MS,
	wait: (ms: number) => Promise<void> = (ms) => new Promise((done) => setTimeout(done, ms))
): Promise<ButtonOutcome> => {
	let waited = 0;
	for (;;) {
		let result: ActionResult;
		try {
			result = await send();
		} catch {
			return "unreached";
		}
		if (!mayClear(result)) return result.status === "error" ? "unreached" : "sent";
		if (waited >= patience) return "unreached";
		await wait(RETRY_MS);
		waited += RETRY_MS;
	}
};

/** What a button finds while the request's choices aren't in yet, so it tries again. */
const NOT_YET: ActionResult = {
	status: "error",
	error: {
		kind: "not_found",
		get what() {
			return t("This request");
		},
	},
};

/**
 * Denies a push's request from its button. The choices come with the push
 * when the extension opened it, or else from the view once it's in, so the
 * answer waits for the view as it waits for the link. A request the view
 * holds with no choices is one only its card may answer, so the phone says
 * to open MaxHQ straight away.
 */
export const denyFromButton = (
	target: PushTarget,
	optionsInView: () => readonly AttentionOption[] | undefined,
	answer: (attention: AttentionId, answer: AttentionAnswer) => Promise<ActionResult>,
	patience?: number,
	wait?: (ms: number) => Promise<void>
): Promise<ButtonOutcome> => {
	const { attention } = target;
	if (!attention) return Promise.resolve("unreached");
	return answerFromButton(
		async () => {
			const options = target.options.length > 0 ? target.options : optionsInView();
			if (!options) return NOT_YET;
			const picked = denyAnswer(options);
			return picked
				? answer(attention, picked)
				: { status: "error", error: { kind: "failed", message: t("No choice fits.") } };
		},
		patience,
		wait
	);
};

/** The note that says a button's answer didn't go. */
export const UNREACHED_TITLE = "Open MaxHQ to answer";

/** A notification still in Notification Center, and what it points at. */
export type Presented = { readonly id: string; readonly target: PushTarget | null };

/**
 * The notifications that are done with: every one for the agent on
 * screen, and each prompt no longer open. A prompt counts as answered only
 * once its agent's device has synced, so a phone that can't reach the
 * device, or hasn't heard from it yet, keeps it.
 */
export const handledNotifications = (
	presented: readonly Presented[],
	view: ViewState,
	shown: AgentId | null,
	now: Timestamp
): string[] =>
	presented.flatMap(({ id, target }) => {
		if (!target) return [];
		if (target.agent === shown) return [id];
		if (target.attention === null || view.tables.attention.has(target.attention)) return [];
		return agentSynced(view, target.agent, now) ? [id] : [];
	});

/**
 * Whether a push that comes while the app is in front is about a prompt
 * the live view holds. That one gets the app's own haptic and banner, so
 * the system's banner and sound stay away.
 */
export const heardLive = (target: PushTarget | null, view: ViewState): boolean =>
	target?.attention != null && view.tables.attention.has(target.attention);
