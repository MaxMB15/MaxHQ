// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Sidebar card, kept on this Mac for every device: whether busy agents
// fold into a Working shelf, and the choices in every snooze menu. Each
// change saves as you make it.

import { useServices } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { setPrefs, usePrefs } from "@/sidebar/prefs";

import { Part, Row, Section } from "./controls";
import { SnoozePresetsEditor } from "./snooze-presets";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** The card's headings and labels, for the settings search. */
export const SIDEBAR_TERMS: readonly string[] = [
	"Shelves",
	"Working shelf",
	"Fold busy agents into a Working shelf",
	"Snooze options",
];

/** The Sidebar card. */
export const SidebarSettingsPage = () => {
	useI18n();
	const services = useServices();
	const workingShelf = usePrefs((p) => p.workingShelf);
	return (
		<Section title={translate("Sidebar")}>
			<Row
				title={translate("Working shelf")}
				sub={translate(
					"Fold busy agents into a Working shelf. They come back to the top when they need you."
				)}
			>
				<Switch
					aria-label={translate("Working shelf")}
					checked={workingShelf}
					onCheckedChange={(on) => setPrefs(services, { workingShelf: on })}
				/>
			</Row>
			<Part title={translate("Snooze options")}>
				<SnoozePresetsEditor />
			</Part>
		</Section>
	);
};
