// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Act directly switch on the Dispatcher settings page. On, the
// dispatcher makes safe changes at once and leaves a card with Undo under
// its answer. Off, every change waits in its plan until you confirm it. The
// Mac's `maxd` keeps the switch with the rest of its settings and holds the
// dispatcher to it.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { Row, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId } from "@maxhq/shared/bindings";
import { actsDirectly } from "@maxhq/shared/dispatch/action";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import { deviceName } from "@maxhq/shared/view/select";

/** The Changes section, with the switch for `device`'s dispatcher. */
export const ActsDirectlySection = ({ device }: { device: DeviceId }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const known = actsDirectly(view, device);
	// What you just picked, shown until the view moves on from what it had then.
	const [pending, setPending] = useState<{ on: boolean; from: boolean | null } | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const on = pending && pending.from === known ? pending.on : (known ?? false);
	const save = async (value: boolean) => {
		setPending({ on: value, from: known });
		setNote({ text: translate("Saving"), error: false });
		try {
			const failure = failureOf(
				await source.setDeviceSettings({
					device,
					...deviceSettingsOf(view, device),
					dispatcher_acts: value,
				})
			);
			if (failure) setPending(null);
			setNote(
				failure
					? { text: failure, error: true }
					: { text: translate("Saved"), error: false }
			);
		} catch (error) {
			console.warn(translate("Couldn't save the device settings"), error);
			setPending(null);
			setNote({ text: translate("Couldn't save it. Try again."), error: true });
		}
	};
	return (
		<Section title={translate("Changes")}>
			<Row
				title={translate("Act directly")}
				sub={
					known === null
						? translate("{{name}}'s maxd is too old for this. Update it first.", {
								name: deviceName(view, device),
							})
						: translate(
								"The dispatcher changes settings, agents and tasks without asking first. Deleting, stopping busy agents and granting permissions still ask."
							)
				}
			>
				<StatusNote note={note} />
				<Switch
					aria-label={translate("Act directly")}
					checked={on}
					disabled={known === null}
					onCheckedChange={(value) => void save(value)}
				/>
			</Row>
		</Section>
	);
};
