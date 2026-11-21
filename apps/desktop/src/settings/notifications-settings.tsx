// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Mac's notifications: whether they show at all, which kinds of alert
// do, which ones play a sound, a test one to try them out, and the badge
// on the bell and in the Dock. The kinds
// are the phone's switches. The app keeps the settings and applies each
// change as you make it. The line at the top says what macOS allows, and
// reads again whenever the window comes back to the front, since that's
// changed in System Settings. The badge's card hears the same read, since
// macOS hides the Dock badge with notifications off or badges off.

import { useCallback, useEffect, useState } from "react";

import { Switch } from "@/components/ui/switch";
import type { PushKind } from "@maxhq/shared/bindings";
import { PUSH_KINDS, switchKind } from "@maxhq/shared/push/push";
import {
	type NotificationAccess,
	notificationAccess,
	notificationPrefs,
	type NotificationPermission,
	type NotificationPrefs,
	openNotificationSettings,
	sendTestNotification,
	setNotificationPrefs,
} from "@/native/notifications";

import { Row, SECONDARY, Section, StatusNote } from "./controls";
import { DOCK_BADGE_TERMS, DockBadgeSection } from "./dock-badge-settings";
import type { Note } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// The phone's words for a prompt mention its lock screen.
const DETAIL: Partial<Record<PushKind, string>> = {
	get needs_input() {
		return translate("A permission, a question or a secret");
	},
};

const STATUS: Record<NotificationPermission, string> = {
	get not_decided() {
		return translate("macOS hasn't asked about MaxHQ's notifications yet. A test one asks.");
	},
	get denied() {
		return translate("Notifications are turned off for MaxHQ in System Settings.");
	},
	get allowed() {
		return translate("macOS shows MaxHQ's notifications.");
	},
	get unavailable() {
		return translate(
			"This build can't show notifications, since it doesn't run from an app bundle."
		);
	},
};

const said = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** The page's headings and labels, for the settings search. */
export const NOTIFICATIONS_TERMS: readonly string[] = [
	"Show notifications",
	"Send a test notification",
	"System Settings",
	"Alerts",
	...PUSH_KINDS.map((each) => each.label),
	"Sounds",
	"Prompts",
	"Other alerts",
	...DOCK_BADGE_TERMS,
];

/** The Notifications page. */
export const NotificationsSettingsPage = () => {
	useI18n();
	const [prefs, setPrefs] = useState<NotificationPrefs | null>(null);
	const [access, setAccess] = useState<NotificationAccess | null>(null);
	const [note, setNote] = useState<Note | null>(null);

	useEffect(() => {
		let live = true;
		void notificationPrefs().then((read) => live && setPrefs(read));
		return () => {
			live = false;
		};
	}, []);

	const check = useCallback(() => {
		void notificationAccess()
			.then(setAccess)
			.catch(() => setAccess(null));
	}, []);
	useEffect(() => {
		check();
		window.addEventListener("focus", check);
		return () => window.removeEventListener("focus", check);
	}, [check]);

	const change = (next: Partial<NotificationPrefs>) => {
		if (!prefs) return;
		const changed = { ...prefs, ...next };
		setPrefs(changed);
		setNote(null);
		setNotificationPrefs(changed).catch((error: unknown) =>
			setNote({ text: translate("Not saved: {{said}}", { said: said(error) }), error: true })
		);
	};

	const openSettings = () => {
		openNotificationSettings().catch((error: unknown) =>
			setNote({ text: said(error), error: true })
		);
	};

	const test = () => {
		setNote(null);
		sendTestNotification()
			.then(check)
			.catch((error: unknown) => setNote({ text: said(error), error: true }));
	};

	const off = !prefs?.show;
	const permission = access?.permission;
	return (
		<>
			<Section title={translate("Notifications")}>
				<Row
					title={
						permission ? (
							<span role="status">{STATUS[permission]}</span>
						) : (
							translate("Checking with macOS…")
						)
					}
				>
					{permission === "denied" ? (
						<button type="button" className={SECONDARY} onClick={openSettings}>
							{translate("Open System Settings")}
						</button>
					) : null}
				</Row>
				<Row
					title={translate("Show notifications")}
					sub={translate("Alerts from your agents, in Notification Center")}
				>
					<Switch
						aria-label={translate("Show notifications")}
						checked={prefs?.show ?? false}
						disabled={!prefs}
						onCheckedChange={(on) => change({ show: on })}
					/>
				</Row>
				<Row
					title={translate("Test notification")}
					sub={
						note ? (
							<StatusNote note={note} />
						) : (
							translate("Shows one whichever kinds are on")
						)
					}
				>
					<button
						type="button"
						className={SECONDARY}
						disabled={permission === "unavailable"}
						onClick={test}
					>
						{translate("Send a test notification")}
					</button>
				</Row>
			</Section>
			<Section
				title={translate("Alerts")}
				hint={translate("The same kinds as on your phones")}
			>
				{PUSH_KINDS.map(({ kind, label, detail }) => (
					<Row key={kind} title={label} sub={DETAIL[kind] ?? detail}>
						<Switch
							aria-label={label}
							checked={prefs?.kinds.includes(kind) ?? false}
							disabled={off}
							onCheckedChange={(on) =>
								prefs && change({ kinds: switchKind(prefs.kinds, kind, on) })
							}
						/>
					</Row>
				))}
			</Section>
			<Section title={translate("Sounds")}>
				<Row
					title={translate("Prompts")}
					sub={translate("An alert that waits on you: needs input, or an error")}
				>
					<Switch
						aria-label={translate("Sound for prompts")}
						checked={prefs?.prompt_sound ?? false}
						disabled={off}
						onCheckedChange={(on) => change({ prompt_sound: on })}
					/>
				</Row>
				<Row title={translate("Other alerts")} sub={translate("Finished, and limits")}>
					<Switch
						aria-label={translate("Sound for other alerts")}
						checked={prefs?.other_sound ?? false}
						disabled={off}
						onCheckedChange={(on) => change({ other_sound: on })}
					/>
				</Row>
			</Section>
			<DockBadgeSection access={access} onOpenSettings={openSettings} />
		</>
	);
};
