// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One device's settings, opened from its page, as the desktop's device
// settings show them: the mode and effort new agents start at, the inbox,
// usage limits, placement, idle agents, memory and setup-tokens, a Mac's
// sleep and Cursor usage, an Incus host's new workspaces, the skill
// library, network access with its phones, the ACP agents, and what only
// the Mac changes. Its `maxd` keeps them, and a save sends them all. The
// Mac lets this phone change them only with its switch on for the device;
// without it every control shows but stays off.

import { useI18n } from "@maxhq/shared/i18n";

import { ScrollView, StyleSheet, View } from "react-native";

import { useNow } from "@maxhq/shared/lib/clock";
import { settingsWhy } from "@maxhq/shared/settings/device-settings";
import { linkForDevice } from "@maxhq/shared/view/select";
import { isLive } from "@maxhq/shared/workspaces/state";

import type { ScreenProps } from "../../navigation";
import { useView } from "../../services";
import { color } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { Muted, type SectionProps } from "../devices/section";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { AcpAgents } from "./AcpAgents";
import {
	IdleCard,
	InboxCard,
	LibraryCard,
	LimitsCard,
	MacCards,
	MemoryCard,
	NewAgentsCard,
	PlacementCard,
	SetupTokensCard,
	WorkspacesCard,
} from "./device-cards";
import { NetworkCard, OnTheMacCard } from "./device-network";
import { type CardProps, useDeviceSave } from "./device-parts";
import { someCards } from "./device-settings";

const Cards = (section: SectionProps) => {
	const { view, device, name, live } = section;
	const lets = useLetsPhone(device);
	const saver = useDeviceSave(view, device);
	const why = settingsWhy(view, device, name, live);
	const some = someCards(view, device);
	const props: CardProps = { ...section, lets, why, saver };
	return (
		<>
			{lets ? null : <NeedsSwitch />}
			<NewAgentsCard {...props} />
			<InboxCard {...props} />
			<LimitsCard {...props} />
			<PlacementCard {...props} />
			<IdleCard {...props} />
			<MemoryCard {...props} />
			<SetupTokensCard {...props} />
			{some.mac ? <MacCards {...props} /> : null}
			{some.workspaces ? <WorkspacesCard {...props} /> : null}
			{some.library ? <LibraryCard {...props} /> : null}
			{some.network ? <NetworkCard {...props} /> : null}
			<AcpAgents {...section} />
			<OnTheMacCard appKeys={some.appKeys} />
		</>
	);
};

export const DeviceSettingsScreen = ({ navigation, route }: ScreenProps<"DeviceSettings">) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const { device } = route.params;
	const link = linkForDevice(view, device);
	const back = { label: t("Back"), onPress: () => navigation.goBack() };
	if (!link) {
		return (
			<View style={styles.screen}>
				<Header title={t("Devices")} back={back} />
				<View style={styles.content}>
					<Muted>{t("This device is gone.")}</Muted>
				</View>
			</View>
		);
	}
	return (
		<View style={styles.screen}>
			<Header title={link.name} back={back} />
			<ConnectionBanner />
			<ScrollView
				contentContainerStyle={styles.content}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="on-drag"
				automaticallyAdjustKeyboardInsets
			>
				<Cards
					device={device}
					name={link.name}
					live={isLive(view, device, now)}
					view={view}
				/>
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
});
