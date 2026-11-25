// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The switches for which pushes this phone gets, and a row saying when
// the system keeps them all off, in iOS's or Android's words.

import { useI18n } from "@maxhq/shared/i18n";

import { Linking } from "react-native";
import { useStore } from "zustand";

import { PUSH_KINDS, switchKind } from "@maxhq/shared/push/push";

import { platform } from "../platform";
import { Button } from "../ui/Button";
import { Card, CardRow } from "../ui/Card";
import { Toggle } from "../ui/Toggle";
import { keepKinds, pushStore } from "./kinds";
import { askToNotify, permissionStore } from "./permission";

const PermissionRow = () => {
	const { t } = useI18n();

	const status = useStore(permissionStore, (state) => state.status);
	if (status === "denied") {
		return (
			<CardRow
				label={t("Notifications are off for MaxHQ")}
				note={t("Turn them on in iOS Settings.", { context: platform.context })}
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
			<CardRow
				label={t("Notifications aren't on yet")}
				note={t("iOS asks once.", { context: platform.context })}
			>
				<Button label={t("Turn on")} small onPress={askToNotify} />
			</CardRow>
		);
	}
	return null;
};

export const PushSection = () => {
	const { t } = useI18n();

	const kinds = useStore(pushStore, (state) => state.kinds);
	return (
		<Card
			title={t("Notifications")}
			note={t("Every paired device sends these. None arrive for the agent on screen.")}
		>
			<PermissionRow />
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
