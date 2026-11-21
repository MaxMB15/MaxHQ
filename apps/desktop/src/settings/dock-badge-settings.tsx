// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Badge card in Notifications: whether the bell and MaxHQ's icon in
// the Dock show how many agents need you, and what counts. Each change
// applies and saves as you make it. macOS hides the Dock badge of an app
// whose notifications or badges are off in System Settings, so the card
// says so, from its read of what macOS allows, and links there.

import { Switch } from "@/components/ui/switch";
import { setBadgePrefs, useBadgePrefs } from "@/native/dock-badge";
import { type NotificationAccess, openNotificationSettings } from "@/native/notifications";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { BADGE_REASON_TEXT, BADGE_REASONS, switchReason } from "@maxhq/shared/view/badge";

import { Row, SECONDARY, Section } from "./controls";
import { useNotificationAccess } from "./notification-access";

/** The card's headings and labels, for the settings search. */
export const DOCK_BADGE_TERMS: readonly string[] = [
	"Badge",
	"Dock badge",
	"Show the badge",
	...BADGE_REASONS.map((reason) => BADGE_REASON_TEXT[reason].label),
];

/** Why macOS hides the Dock badge, or null when nothing does or it isn't known yet. */
const hiddenBecause = (access: NotificationAccess | null): string | null => {
	if (access?.permission === "denied" || access?.permission === "not_decided") {
		return translate("macOS hides the Dock badge until MaxHQ's notifications are allowed.");
	}
	if (access && !access.badge) {
		return translate(
			"Badges are turned off for MaxHQ in System Settings, so macOS hides the count in the Dock."
		);
	}
	return null;
};

/** The Badge card. */
export const DockBadgeSection = () => {
	useI18n();
	const { access } = useNotificationAccess();
	const onOpenSettings = () => {
		openNotificationSettings().catch((error: unknown) =>
			console.warn(translate("Couldn't open System Settings"), error)
		);
	};
	const show = useBadgePrefs((p) => p.show);
	const counts = useBadgePrefs((p) => p.counts);
	// It only matters while MaxHQ would show a badge.
	const hidden = show ? hiddenBecause(access) : null;
	return (
		<Section title={translate("Badge")} hint={translate("Each agent counts once")}>
			<Row
				title={translate("Show the badge")}
				sub={translate(
					"How many agents need you, on the bell and on MaxHQ's icon in the Dock"
				)}
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
