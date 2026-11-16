// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Every device this phone pairs with, in a card under each Mac's name as
// the desktop's devices list: how it is, its agents, whether the phone
// reaches it now, and by which route, with the others it tries. A device
// that ended the phone's sign-in offers to pair again. Showing one Mac at a
// time, only that Mac's card shows.

import { useI18n } from "@maxhq/shared/i18n";

import { ScrollView, StyleSheet, Text, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import type { ConnectionLook } from "@maxhq/shared/status/connection";
import { type DeviceReach, deviceReach } from "@maxhq/shared/status/reach";
import type { ViewState } from "@maxhq/shared/view/replica";

import type { ScreenProps } from "../../navigation";
import { useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { ConnectionDot } from "../../ui/Dot";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { Plus } from "../../ui/Icon";
import { Pill } from "../../ui/Pill";
import { deviceGroups } from "../macs/macs";
import { useMacs } from "../macs/useMacs";
import { agentSummary, reachSummary } from "./summary";

const LOOK: Record<ConnectionLook, string> = {
	live: color.readyText,
	asleep: color.mutedForeground,
	connecting: color.connectingText,
	agent_down: color.linked,
	unreachable: color.errorText,
	unknown: color.mutedForeground,
};

export const DevicesScreen = ({ navigation }: ScreenProps<"Devices">) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(10_000);
	const all = deviceReach(view, now);
	const macs = useMacs();
	const groups = deviceGroups(all, macs.macs, macs.mode);
	const devices = groups.flatMap((group) => group.devices);
	const pair = () => navigation.navigate("Scan");
	return (
		<View style={styles.screen}>
			<Header
				title={t("Devices")}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content}>
				{all.length === 0 ? (
					<View style={styles.empty}>
						<Text style={styles.note}>
							{t("This phone isn't paired with a device yet.")}
						</Text>
						<Button label={t("Pair with a device")} tone="primary" onPress={pair} />
					</View>
				) : (
					<>
						<Text style={styles.summary}>{reachSummary(devices)}</Text>
						{groups.map((group) => (
							<Card key={group.key} title={group.name}>
								{group.devices.map((device, i) => (
									<DeviceRow
										key={device.id}
										device={device}
										view={view}
										first={i === 0}
										pair={pair}
									/>
								))}
							</Card>
						))}
						<Button
							label={t("Pair with another device")}
							icon={<Plus size={12} />}
							onPress={pair}
						/>
					</>
				)}
			</ScrollView>
		</View>
	);
};

const DeviceRow = ({
	device,
	view,
	first,
	pair,
}: {
	device: DeviceReach;
	view: ViewState;
	first: boolean;
	pair: () => void;
}) => {
	const { t } = useI18n();

	const agents = agentSummary(view, device.id);
	return (
		<View
			style={[styles.row, !first && styles.line]}
			accessible
			accessibilityLabel={`${device.name}. ${device.title}. ${device.detail}${device.version ? `. ${device.version}` : ""}`}
		>
			<View style={styles.top}>
				<ConnectionDot look={device.look} size={8} surface={color.card} decorative />
				<Text style={styles.name} numberOfLines={1}>
					{device.name}
				</Text>
				<Text style={[styles.status, { color: LOOK[device.look] }]} numberOfLines={1}>
					{device.title}
				</Text>
			</View>
			<View style={styles.body}>
				{agents ? <Text style={styles.agents}>{agents}</Text> : null}
				{device.detail ? <Text style={styles.note}>{device.detail}</Text> : null}
				<Text style={styles.note}>
					{device.via
						? t("Reached now: {{route}}", { route: device.via })
						: t("Not reached right now")}
				</Text>
				{device.version ? <Text style={styles.note}>{device.version}</Text> : null}
				{device.routes.length > 1 ? (
					<View style={styles.routes}>
						{device.routes.map((route) => (
							<Pill
								key={route.label}
								label={route.label}
								mono
								tone={route.current ? "primary" : "plain"}
								note={
									route.current
										? t("tried first")
										: route.lastGoodHere
											? t("worked on this network")
											: undefined
								}
							/>
						))}
					</View>
				) : null}
				{device.pairAgain ? (
					<View style={styles.again}>
						<Button label={t("Scan a new code")} tone="primary" onPress={pair} />
					</View>
				) : null}
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12 },
	empty: { gap: 12 },
	summary: { color: color.mutedForeground, fontSize: size.meta, paddingHorizontal: 2 },
	row: { minHeight: 44, paddingHorizontal: 14, paddingVertical: 12, gap: 4 },
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	top: { flexDirection: "row", alignItems: "center", gap: 9 },
	name: { flexShrink: 1, color: color.foregroundStrong, fontSize: size.body, fontWeight: "600" },
	status: { marginLeft: "auto", fontSize: size.meta },
	body: { paddingLeft: 17, gap: 3 },
	agents: { color: color.secondaryForeground, fontSize: size.meta },
	note: { color: color.mutedForeground, fontSize: size.meta },
	routes: { flexDirection: "row", flexWrap: "wrap", gap: 5, marginTop: 4 },
	again: { alignItems: "flex-start", marginTop: 6 },
});
