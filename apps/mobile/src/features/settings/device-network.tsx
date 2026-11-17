// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Whether phones reach a device over the network, on which port and at
// which addresses, and the phones signed in to it there, each with Revoke.
// Then what only the Mac changes: its app keys and its device links.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import {
	DEFAULT_PORT,
	parsePort,
	sessionLine,
	signedIn,
	type SignedIn,
} from "@maxhq/shared/settings/network-access";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { CardBody, NoteLine, useAction } from "../devices/section";
import { type CardProps, isOff, NumberRow, PartNote, SwitchRow } from "./device-parts";

/** A phone signed in to the device over the network, with its Revoke. */
const PhoneSessionRow = ({ signed }: { signed: SignedIn }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const now = useNow(60_000);
	const { busy, note, run } = useAction();
	const revoke = () =>
		confirm(
			{ title: signed.name, detail: t("The phone has to pair again to reach it.") },
			t("Revoke"),
			() => void run(null, () => source.revokePhoneSession(signed.session.id))
		);
	return (
		<>
			<CardRow label={signed.name} note={sessionLine(signed.session, now)}>
				<Button small tone="danger" label={t("Revoke")} disabled={busy} onPress={revoke} />
			</CardRow>
			{note ? (
				<CardBody>
					<NoteLine note={note} />
				</CardBody>
			) : null}
		</>
	);
};

export const NetworkCard = ({ view, device, name, lets, why, saver }: CardProps) => {
	const { t } = useI18n();
	const { shown, save, noteFor, setNote } = saver;
	const access = view.tables.network_access.get(device);
	if (!access) return null;
	const off = isOff(why.offline, lets);
	return (
		<Card title={t("Network access")} note={t("Off by default")}>
			<SwitchRow
				label={t("Network access")}
				sub={
					why.offline ??
					t("Phones on the same network or tailnet can reach {{name}} while it's on.", {
						name,
					})
				}
				value={shown.network.on ?? false}
				off={off}
				onChange={(on) => void save("network", { network: { ...shown.network, on } })}
			/>
			{access.problem ? (
				<CardBody>
					<Text style={styles.warn}>{access.problem}</Text>
				</CardBody>
			) : null}
			<NumberRow
				label={t("Port")}
				sub={
					why.offline ??
					t("The port phones reach {{name}} on. {{DEFAULT_PORT}} by default.", {
						name,
						DEFAULT_PORT,
					})
				}
				value={shown.network.port ?? DEFAULT_PORT}
				off={off}
				onCommit={(text) => {
					const parsed = parsePort(text);
					if (parsed.ok)
						void save("network", { network: { ...shown.network, port: parsed.port } });
					else setNote("network", { text: parsed.reason, error: true });
				}}
			/>
			<PartNote note={noteFor("network")} />
			{access.on && access.hosts.length > 0 ? (
				<CardRow label={t("Addresses for phones")} note={access.hosts.join(", ")} />
			) : null}
			{signedIn(view, device).map((signed) => (
				<PhoneSessionRow key={signed.session.id} signed={signed} />
			))}
		</Card>
	);
};

/** App keys and device links, which only the Mac changes, so they say so. */
export const OnTheMacCard = ({ appKeys }: { appKeys: boolean }) => {
	const { t } = useI18n();
	return (
		<Card label={t("On the Mac")}>
			{appKeys ? (
				<CardRow first label={t("App keys")} note={t("Apps that may do everything here")}>
					<Text style={styles.value}>{t("On the Mac")}</Text>
				</CardRow>
			) : null}
			<CardRow
				first={!appKeys}
				label={t("Device links")}
				note={t("Lets agents here reach agents on the others")}
			>
				<Text style={styles.value}>{t("On the Mac")}</Text>
			</CardRow>
		</Card>
	);
};

const styles = StyleSheet.create({
	warn: { color: color.busyText, fontSize: size.meta },
	value: { color: color.secondaryForeground, fontSize: size.small },
});
