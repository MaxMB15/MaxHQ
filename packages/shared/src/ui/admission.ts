// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The cards a device raises when a program connects without a key it trusts
// and asks for more than such a connection may do: which are open, what
// each says, and its answers. Only a proven desktop app answers one. A phone
// shows it and says to answer on the Mac, since only the Mac lets things in.

import { t } from "../i18n";

import type { Admission, AdmissionId, ConsentFact, DeviceId, LinkId } from "../bindings";
import { all, type ViewState } from "../view/replica";
import { deviceName, linkForDevice } from "../view/select";

/** What a phone shows in place of the answers. */
export const ANSWER_ON_THE_MAC = "Answer this on the Mac";

/** The answer that keeps the program out, which every card offers first. */
export const DONT_ALLOW = "Don't allow";

/** What the card says, laid out. */
export type AdmissionCard = {
	readonly id: AdmissionId;
	readonly device: DeviceId;
	/** Your name for the device. */
	readonly deviceName: string;
	readonly title: string;
	/** The exact risk, in a sentence or two. */
	readonly risk: string;
	/** What the device knows about the program, in order. */
	readonly facts: readonly ConsentFact[];
	/** The answer that keeps it out, which comes first. */
	readonly deny: string;
	/** The answer that lets it in. */
	readonly allow: string;
	/** What a yes keeps, and for how long. */
	readonly remember: string;
};

/**
 * The open cards, oldest first, on `device`, or on every device with none.
 * Each device drops its own cards when they're answered, run out or their
 * program leaves, so a card here is open by its device's clock.
 */
export const openAdmissions = (view: ViewState, device: DeviceId | null = null): Admission[] =>
	all(view, "admission")
		.filter((card) => device === null || card.device === device)
		.sort((a, b) => a.raised_at - b.raised_at || a.id.localeCompare(b.id));

/** The answer that lets the program in: the one that isn't Don't allow. */
const allowOf = (admission: Admission): string =>
	admission.question.options.find((option) => option.label !== DONT_ALLOW)?.label ??
	t("Allow until it disconnects");

/** Whether a yes trusts the program's key, rather than one connection. */
const trustsKey = (admission: Admission): boolean =>
	(admission.question.consent?.facts ?? []).some((fact) => fact.label === "Key");

/** The card's words, from its consent, or from its plain text without one. */
export const admissionCard = (view: ViewState, admission: Admission): AdmissionCard => {
	const name = deviceName(view, admission.device);
	const consent = admission.question.consent;
	const [title = t("Let this program control {{name}}?", { name: name }), ...rest] =
		admission.question.text.split("\n\n");
	return {
		id: admission.id,
		device: admission.device,
		deviceName: name,
		title: consent?.title ?? title,
		risk: consent?.risk ?? rest.join("\n\n"),
		facts: consent?.facts ?? [],
		deny: DONT_ALLOW,
		allow: allowOf(admission),
		remember: trustsKey(admission)
			? t("A yes adds the key to App keys in {{name}}'s settings, until you remove it.", {
					name: name,
				})
			: t("Nothing is kept. If it connects again, it asks again."),
	};
};

/** The link that opens the card's device page, if one reaches the device. */
export const admissionLink = (view: ViewState, admission: Admission): LinkId | null =>
	linkForDevice(view, admission.device)?.id ?? null;
