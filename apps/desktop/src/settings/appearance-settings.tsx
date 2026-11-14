// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { setThemeChoice, useThemeChoice } from "@/app/theme";
import { t, type LanguageChoice, useI18n } from "@maxhq/shared/i18n";
import type { ThemeChoice } from "@maxhq/shared/theme/theme";

import { Row, Section } from "./controls";
import { type Choice, Pick as Picker } from "./snooze-presets";

const LANGUAGE_OPTIONS: readonly Choice<LanguageChoice>[] = [
	{
		value: "en",
		get label() {
			return t("English");
		},
	},
	{
		value: "nl",
		get label() {
			return t("Nederlands");
		},
	},
	{
		value: "de",
		get label() {
			return t("Deutsch");
		},
	},
	{
		value: "fr",
		get label() {
			return t("Français");
		},
	},
	{
		value: "es",
		get label() {
			return t("Español");
		},
	},
];

/** The page's headings and labels, for the settings search. */
export const APPEARANCE_TERMS: readonly string[] = [
	"Colors",
	"Theme",
	"System",
	"Light",
	"Dark",
	"Language",
	"Interface language",
	...LANGUAGE_OPTIONS.map((option) => option.label),
];

/** The Appearance page. */
export const AppearanceSettingsPage = () => {
	const { t, choice: languageChoice, setLanguageChoice } = useI18n();
	const choice = useThemeChoice();
	const themes: readonly Choice<ThemeChoice>[] = [
		{ value: "system", label: t("System") },
		{ value: "light", label: t("Light") },
		{ value: "dark", label: t("Dark") },
	];
	return (
		<>
			<Section title={t("Colors")}>
				<Row
					title={t("Theme")}
					sub={t(
						"Light or dark. System follows the Mac's appearance as it changes through the day."
					)}
				>
					<Picker
						label={t("Theme")}
						value={choice}
						options={themes}
						onChange={setThemeChoice}
						className="w-[180px]"
					/>
				</Row>
			</Section>
			<Section title={t("Language")}>
				<Row
					title={t("Interface language")}
					sub={t("System follows this device's language. Changes apply immediately.")}
				>
					<Picker
						label={t("Language")}
						value={languageChoice}
						options={[{ value: "system", label: t("System") }, ...LANGUAGE_OPTIONS]}
						onChange={setLanguageChoice}
						className="w-[180px]"
					/>
				</Row>
			</Section>
		</>
	);
};
