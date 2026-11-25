// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The switches for which pushes this phone gets, and rows saying when the
// system keeps them all off, when the build gets none, and which devices
// need a newer maxd for them, in iOS's or Android's words.

import { useI18n } from "@maxhq/shared/i18n";

import { Linking } from "react-native";
import { useStore } from "zustand";

import { PUSH_KINDS, pushUpdateNote, switchKind } from "@maxhq/shared/push/push";

import { platform } from "../platform";
import { useView } from "../services";
import { Button } from "../ui/Button";
import { Card, CardRow } from "../ui/Card";
import { Toggle } from "../ui/Toggle";
import { keepKinds, pushStore } from "./kinds";
import { askToNotify, permissionStore, pushTopic } from "./permission";

const PermissionRow = () => {
	const { t } = useI18n();

	const status = useStore(permissionStore, (state) => state.status);
	if (status === "denied") {
		return (
			<CardRow
				label={t("Notifications are off for MaxHQ")}
				note={t("Turn them on in iOS Settings.")}
			>
				<Button
					label={t("Open Settings")}
					small
					onPress={() => void Linking.openSettings()}
				/>
			</CardRow>
		);
	}
	if (status === "undetermined") {
		return (
			<CardRow label={t("Notifications aren't on yet")} note={t("iOS asks once.")}>
				<Button label={t("Turn on")} small onPress={askToNotify} />
			</CardRow>
		);
	}
	return null;
};

/** Says so when the build can't get pushes, as one run from source without signing or Firebase. */
const BuildRow = () => {
	const { t } = useI18n();

	if (pushTopic()) return null;
	return (
		<CardRow
			label={t("This build gets no pushes")}
			note={t("It was built without the signing that push needs.")}
		/>
	);
};

/** Names the devices whose maxd can't push to this phone yet. */
const UpdateRow = () => {
	// Rendered again when the language changes, so the note follows it.
	useI18n();

	const view = useView((state) => state.view);
	const note = pushUpdateNote(view, platform.pushService);
	return note ? <CardRow label={note} /> : null;
};

export const PushSection = () => {
	const { t } = useI18n();

	const kinds = useStore(pushStore, (state) => state.kinds);
	return (
		<Card
			title={t("Notifications")}
			note={t("Every paired device sends these. None arrive for the agent on screen.")}
		>
			<BuildRow />
			<PermissionRow />
			<UpdateRow />
			{PUSH_KINDS.map(({ kind, label, detail }) => (
				<CardRow key={kind} label={label} note={detail}>
					<Toggle
						label={label}
						value={kinds.includes(kind)}
						onChange={(on) => keepKinds(switchKind(kinds, kind, on))}
					/>
				</CardRow>
			))}
		</Card>
	);
};
