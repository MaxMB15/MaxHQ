// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Dock badge's card on the Notifications page: whether MaxHQ's icon
// shows how many agents need you, and what counts. Each change applies and
// saves as you make it.

import { Switch } from "@/components/ui/switch";
import { setBadgePrefs, useBadgePrefs } from "@/native/dock-badge";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { BADGE_REASON_TEXT, BADGE_REASONS, switchReason } from "@maxhq/shared/view/badge";

import { Row, Section } from "./controls";

/** The card's headings and labels, for the settings search. */
export const DOCK_BADGE_TERMS: readonly string[] = [
	"Dock badge",
	"Show the badge",
	...BADGE_REASONS.map((reason) => BADGE_REASON_TEXT[reason].label),
];

/** The Dock badge card. */
export const DockBadgeSection = () => {
	useI18n();
	const show = useBadgePrefs((p) => p.show);
	const counts = useBadgePrefs((p) => p.counts);
	return (
		<Section title={translate("Dock badge")} hint={translate("Each agent counts once")}>
			<Row
				title={translate("Show the badge")}
				sub={translate("The number of agents that need you, on MaxHQ's icon in the Dock")}
			>
				<Switch
					aria-label={translate("Show the badge")}
					checked={show}
					onCheckedChange={(on) => setBadgePrefs({ show: on })}
				/>
			</Row>
			{BADGE_REASONS.map((reason) => {
				const { label, detail } = BADGE_REASON_TEXT[reason];
				return (
					<Row key={reason} title={label} sub={detail}>
						<Switch
							aria-label={translate("Badge: {{what}}", { what: label })}
							checked={counts.includes(reason)}
							disabled={!show}
							onCheckedChange={(on) =>
								setBadgePrefs({ counts: switchReason(counts, reason, on) })
							}
						/>
					</Row>
				);
			})}
		</Section>
	);
};
