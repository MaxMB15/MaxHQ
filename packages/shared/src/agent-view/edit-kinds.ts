// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The kinds of file an agent in Accept edits or Auto asks before it
// changes, as git hooks and CI workflows run later with your keys. maxd
// words the card in English; a client that knows the kind words it in the
// reader's language, and Project settings names each kind you remembered.

import { t } from "../i18n";

import type { Consent, EditKind } from "../bindings";

/** What a card and Project settings say about one kind of file. */
export type EditWords = {
	/** The kind's short name, as Project settings lists it. */
	readonly name: string;
	/** The card's title. */
	readonly title: string;
	/** What a change to one can do, in one line. */
	readonly meaning: string;
	/** What remembering it lets agents do, and where to turn it off. */
	readonly remembered: string;
};

/** The words for `kind`, in the reader's language. */
export const editWords = (kind: EditKind): EditWords => {
	switch (kind) {
		case "git_hooks":
			return {
				name: t("Git hooks"),
				title: t("Change git hooks?"),
				meaning: t("Git hooks run on your next commit, with your keys."),
				remembered: t(
					"Agents in Accept edits or Auto may then change your git hooks in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
		case "ci_workflows":
			return {
				name: t("CI workflows"),
				title: t("Change CI workflows?"),
				meaning: t("CI workflows run on your next push, with the repository's secrets."),
				remembered: t(
					"Agents in Accept edits or Auto may then change your CI workflows in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
		case "editor_tasks":
			return {
				name: t("Editor tasks"),
				title: t("Change editor tasks?"),
				meaning: t("Editor tasks run as you when your editor starts them."),
				remembered: t(
					"Agents in Accept edits or Auto may then change your editor tasks in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
		case "package_hooks":
			return {
				name: t("Package manager hooks"),
				title: t("Change package manager hooks?"),
				meaning: t("Package manager hooks run as you when packages install."),
				remembered: t(
					"Agents in Accept edits or Auto may then change your package manager hooks in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
		case "git_settings":
			return {
				name: t("Git settings"),
				title: t("Change git settings?"),
				meaning: t(
					"Git settings pick the submodules git fetches and the filters it runs on your files."
				),
				remembered: t(
					"Agents in Accept edits or Auto may then change your git settings in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
		case "dev_container":
			return {
				name: t("Dev container"),
				title: t("Change the dev container?"),
				meaning: t(
					"The dev container sets what runs when it builds and starts, with your files."
				),
				remembered: t(
					"Agents in Accept edits or Auto may then change your dev container in this project without asking. Turn it off in Project settings, under Permissions."
				),
			};
	}
};

/** maxd's English for the card's parts that aren't the kind's own words. */
const FILE = "File";
const REMEMBER = "Remember for this project";
const ALLOW = "Allow";
const DENY = "Deny";

/**
 * A consent card in the reader's language where it asks about a kind of
 * file, and as maxd worded it otherwise.
 */
export const shownConsent = (consent: Consent): Consent => {
	if (!consent.edit) return consent;
	const words = editWords(consent.edit);
	return {
		...consent,
		title: words.title,
		risk: words.meaning,
		facts: consent.facts?.map((fact) =>
			fact.label === FILE ? { ...fact, label: t("File") } : fact
		),
		remember: consent.remember && {
			...consent.remember,
			label:
				consent.remember.label === REMEMBER
					? t("Remember for this project")
					: consent.remember.label,
			settings: words.remembered,
		},
	};
};

/**
 * What an option's button says. The answer still sends maxd's label, which
 * is how maxd knows the option.
 */
export const shownOption = (consent: Consent, label: string): string => {
	if (!consent.edit) return label;
	if (label === ALLOW) return t("Allow");
	if (label === DENY) return t("Deny");
	return label;
};
