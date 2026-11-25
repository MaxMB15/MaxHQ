// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Two cards kept on this Mac, since editors and keyboards belong to it:
// Open in, which editor the Open in button and file rows' buttons use, or
// none, and Composer, what Enter does while an agent works and how the
// boxes you type prompts and notes in check spelling. Each change saves as
// you make it.

import { useServices } from "@/app/services";
import { setDefaultEditor, useDefaultEditor } from "@/agent-view/editor-prefs";
import { useEditors } from "@/agent-view/open-in";
import { type DefaultEditor, editorOptions, LAST_USED } from "@maxhq/shared/editor/open";
import { Switch } from "@/components/ui/switch";
import { commandEnter } from "@/composer/footer";
import { useComposerNative } from "@/composer/native";
import { setTypingPrefs, useTypingPrefs } from "@/composer/typing-prefs";
import { BUSY_SEND_WORDS, type BusySend } from "@maxhq/shared/composer/send";

import { Row, Section } from "./controls";
import { type Choice, Pick as Picker } from "./snooze-presets";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// The picker's values: "last", "none", or "editor:" and an editor's id.
const valueOf = (choice: DefaultEditor): string =>
	choice.kind === "editor" ? `editor:${choice.id}` : choice.kind;

const choiceOf = (value: string): DefaultEditor => {
	if (value === "none") return { kind: "none" };
	if (value.startsWith("editor:")) return { kind: "editor", id: value.slice("editor:".length) };
	return LAST_USED;
};

/** The Open in card's labels, for the settings search. */
export const OPEN_IN_TERMS: readonly string[] = [
	"Editor",
	"Default editor",
	"Last used",
	"Cursor",
	"VS Code",
];

/** The Composer card's labels, for the settings search. */
export const COMPOSER_TERMS: readonly string[] = [
	"Typing",
	"While an agent works, Enter",
	"Queue",
	"Steer",
	"Check spelling",
	"Correct spelling automatically",
];

/** The Open in card. */
export const OpenInSettings = () => {
	useI18n();
	const services = useServices();
	const fixed = useDefaultEditor();
	const editors = useEditors();
	const options: Choice<string>[] = [
		{ value: valueOf(LAST_USED), label: translate("Last used") },
		...editorOptions(editors).map((e) => ({
			value: valueOf({ kind: "editor", id: e.id }),
			label: e.found ? e.label : translate("{{label}}, not found", { label: e.label }),
		})),
		{ value: valueOf({ kind: "none" }), label: translate("None") },
	];
	return (
		<Section title={translate("Open in")}>
			<Row
				title={translate("Default editor")}
				sub={translate(
					"Which editor the Open in button and the buttons on file rows use. Last used is the one you opened last, and so is a set editor this Mac doesn't have. None hides those buttons."
				)}
			>
				<Picker
					label={translate("Default editor")}
					value={valueOf(fixed)}
					options={options}
					onChange={(value) => setDefaultEditor(services, choiceOf(value))}
					className="w-[180px]"
				/>
			</Row>
		</Section>
	);
};

// What Command-Enter does instead, after the pick's own line.
const otherKey = (pick: BusySend): string =>
	pick === "queue"
		? translate("{{keyHint}} steers instead.", { keyHint: commandEnter() })
		: translate("{{keyHint}} queues instead.", { keyHint: commandEnter() });

/**
 * The Composer card: what Enter does while an agent works, Check spelling,
 * and Correct spelling automatically, which needs it.
 */
export const ComposerSettings = () => {
	useI18n();
	const services = useServices();
	const { checkSpelling, correctSpelling, busySend } = useTypingPrefs();
	// Only macOS fixes words as you type. Elsewhere the switch stays, off,
	// with the reason.
	const corrects = useComposerNative().hasNativeSpelling();
	return (
		<Section title={translate("Composer")}>
			<Row
				title={translate("While an agent works, Enter")}
				sub={`${BUSY_SEND_WORDS[busySend].line} ${otherKey(busySend)}`}
			>
				<Picker
					label={translate("While an agent works, Enter")}
					value={busySend}
					options={(["queue", "steer"] as const).map((value) => ({
						value,
						label: BUSY_SEND_WORDS[value].label,
					}))}
					onChange={(value) => setTypingPrefs(services, { busySend: value })}
					className="w-[180px]"
				/>
			</Row>
			<Row
				title={translate("Check spelling")}
				sub={translate(
					"Underline misspelled words in the composer and in notes, with the Mac's guesses on a right-click."
				)}
			>
				<Switch
					aria-label={translate("Check spelling")}
					checked={checkSpelling}
					onCheckedChange={(on) => setTypingPrefs(services, { checkSpelling: on })}
				/>
			</Row>
			<Row
				title={translate("Correct spelling automatically")}
				sub={
					corrects
						? translate(
								"Let macOS fix words, curl quotes and expand text replacements as you type, by its own keyboard settings. Needs Check spelling."
							)
						: translate("Only on macOS, where the system fixes words as you type.")
				}
			>
				<Switch
					aria-label={translate("Correct spelling automatically")}
					checked={corrects && checkSpelling && correctSpelling}
					disabled={!corrects || !checkSpelling}
					onCheckedChange={(on) => setTypingPrefs(services, { correctSpelling: on })}
				/>
			</Row>
		</Section>
	);
};
