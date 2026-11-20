// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A chip for each test machine an agent holds, with its kind and time
// left, opening the lease's own screen. As a bar it sits under a screen's
// header, padded and with a line under it, and shows nothing without a lease.

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useMemo } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";

import type { AgentId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { leaseChip, leaseLine } from "@maxhq/shared/test-hosts/format";
import { leasesOf } from "@maxhq/shared/test-hosts/hosts";

import type { Screens } from "../../navigation";
import { useView } from "../../services";
import { color, radius, size } from "../../theme";

const SLOP = { top: 11, bottom: 11, left: 4, right: 4 };

export const LeaseChips = ({ agent, bar }: { agent: AgentId; bar?: boolean }) => {
	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const leases = useMemo(() => leasesOf(view, { kind: "agent", agent }), [view, agent]);
	if (leases.length === 0) return null;
	return (
		<View style={[styles.row, bar && styles.bar]}>
			{leases.map((lease) => (
				<Pressable
					key={lease.id}
					accessibilityRole="button"
					accessibilityLabel={leaseChip(lease, now)}
					accessibilityHint={leaseLine(lease, now)}
					hitSlop={SLOP}
					onPress={() =>
						navigation.navigate("Lease", { device: lease.device, lease: lease.id })
					}
					style={({ pressed }) => [styles.chip, pressed && styles.pressed]}
				>
					<Text numberOfLines={1} style={styles.text}>
						{leaseChip(lease, now)}
					</Text>
				</Pressable>
			))}
		</View>
	);
};

const styles = StyleSheet.create({
	row: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	bar: {
		paddingHorizontal: 14,
		paddingVertical: 6,
		borderBottomColor: color.border,
		borderBottomWidth: StyleSheet.hairlineWidth,
	},
	chip: {
		height: 22,
		justifyContent: "center",
		paddingHorizontal: 7,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.secondary,
	},
	pressed: { backgroundColor: color.accent },
	text: { color: color.secondaryForeground, fontSize: size.tiny },
});
