// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Some requests a device's maxd takes from a phone only when the Mac turned
// on "Let this phone do everything" for it there, such as starting an
// agent. Screens keep those actions in view, off until the switch is on,
// with a line under them that says where to turn it on.

import { useState } from "react";
import { StyleSheet, Text } from "react-native";

import type { DeviceId, PhoneId } from "@maxhq/shared/bindings";
import { useI18n } from "@maxhq/shared/i18n";
import { letsPhone } from "@maxhq/shared/settings/phones";

import { useServices, useView } from "../../services";
import { color, size } from "../../theme";

/** This phone's ID, or null off the phone's core, where nothing is gated. */
const usePhoneId = (): PhoneId | null => {
	const { phone } = useServices();
	const [id] = useState(() => {
		const key = phone?.phoneKey();
		return key?.status === "ok" ? key.data.phone : null;
	});
	return id;
};

/**
 * Whether `device` takes this phone's gated requests. With no device yet,
 * it doesn't. Off the phone's core, such as with the sample data, it does.
 */
export const useLetsPhone = (device: DeviceId | null): boolean => {
	const { phone } = useServices();
	const id = usePhoneId();
	return useView(
		(s) => device !== null && (phone === null || (id !== null && letsPhone(s.view, id, device)))
	);
};

/** The line under an action the switch keeps off. */
export const NeedsSwitch = () => {
	const { t } = useI18n();
	return (
		<Text style={styles.hint}>
			{t(
				"To do this from the phone, turn on Let this phone do everything for this device in the Mac's Settings, under Phones."
			)}
		</Text>
	);
};

const styles = StyleSheet.create({
	hint: { color: color.mutedForeground, fontSize: size.small, marginTop: 6 },
});
