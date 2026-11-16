// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Dock badge's card on the Notifications page: whether MaxHQ's icon
// shows how many agents need you, and what counts. Each change applies and
// saves as you make it. macOS hides the badge of an app whose
// notifications or badges are off in System Settings, so the card says
// so, from the page's read of what macOS allows, and links there.

import { Switch } from "@/components/ui/switch";
import { setBadgePrefs, useBadgePrefs } from "@/native/dock-badge";
import type { NotificationAccess } from "@/native/notifications";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { BADGE_REASON_TEXT, BADGE_REASONS, switchReason } from "@maxhq/shared/view/badge";

import { Row, SECONDARY, Section } from "./controls";

/** The card's headings and labels, for the settings search. */
export const DOCK_BADGE_TERMS: readonly string[] = [
	"Dock badge",
	"Show the badge",
	...BADGE_REASONS.map((reason) => BADGE_REASON_TEXT[reason].label),
];

/** Why macOS hides the badge, or null when nothing does or it isn't known yet. */
const hiddenBecause = (access: NotificationAccess | null): string | null => {
	if (access?.permission === "denied" || access?.permission === "not_decided") {
		return translate("macOS hides the badge until MaxHQ's notifications are allowed.");
	}
	if (access && !access.badge) {
		return translate(
			"Badges are turned off for MaxHQ in System Settings, so macOS hides the count."
		);
	}
	return null;
};

/** The Dock badge card. */
export const DockBadgeSection = ({
	access,
	onOpenSettings,
}: {
	/** What macOS allows, or null before the page has read it. */
	access: NotificationAccess | null;
	onOpenSettings: () => void;
}) => {
	useI18n();
	const show = useBadgePrefs((p) => p.show);
	const counts = useBadgePrefs((p) => p.counts);
	// It only matters while MaxHQ would show a badge.
	const hidden = show ? hiddenBecause(access) : null;
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
			{hidden ? (
				<Row title={hidden}>
					<button type="button" className={SECONDARY} onClick={onOpenSettings}>
						{translate("Open System Settings")}
					</button>
				</Row>
			) : null}
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
