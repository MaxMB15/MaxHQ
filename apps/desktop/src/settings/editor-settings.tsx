// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The editor settings, kept on this Mac since editors belong to it: which
// editor the Open in button and file rows' buttons use, or none, and how
// the boxes you type prompts and notes in check spelling. Each change saves
// as you make it.

import { useServices } from "@/app/services";
import { setDefaultEditor, useDefaultEditor } from "@/agent-view/editor-prefs";
import { useEditors } from "@/agent-view/open-in";
import { type DefaultEditor, editorOptions, LAST_USED } from "@maxhq/shared/editor/open";
import { Switch } from "@/components/ui/switch";
import { setTypingPrefs, useTypingPrefs } from "@/composer/typing-prefs";

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

/** The page's headings and labels, for the settings search. */
export const EDITOR_TERMS: readonly string[] = [
	"Open in",
	"Default editor",
	"Last used",
	"Cursor",
	"VS Code",
	"Typing",
	"Check spelling",
	"Correct spelling automatically",
];

/** The Editor page. */
export const EditorSettingsPage = () => {
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
		<>
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
			<TypingSection />
		</>
	);
};

/** Check spelling, and Correct spelling automatically, which needs it. */
const TypingSection = () => {
	const services = useServices();
	const { checkSpelling, correctSpelling } = useTypingPrefs();
	return (
		<Section title={translate("Typing")}>
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
				sub={translate(
					"Let macOS fix words, curl quotes and expand text replacements as you type, by its own keyboard settings. Needs Check spelling."
				)}
			>
				<Switch
					aria-label={translate("Correct spelling automatically")}
					checked={checkSpelling && correctSpelling}
					disabled={!checkSpelling}
					onCheckedChange={(on) => setTypingPrefs(services, { correctSpelling: on })}
				/>
			</Row>
		</Section>
	);
};
