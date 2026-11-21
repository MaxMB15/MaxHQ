// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher's own settings on the Dispatcher settings page. Act
// directly: on, the dispatcher makes safe changes at once and leaves a card
// with Undo under its answer; off, every change waits in its plan until you
// confirm it. New chat after a pause: how long after a chat's last message
// your next request starts a fresh one. The Mac's `maxd` keeps both with
// the rest of its settings and holds the dispatcher to them.

import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { Picker } from "@/new-agent/picker";
import { Row, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceId } from "@maxhq/shared/bindings";
import { actsDirectly } from "@maxhq/shared/dispatch/action";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { chatGap, chatGapOptions, DEFAULT_CHAT_GAP_MINUTES } from "@maxhq/shared/settings/chat-gap";
import { type DeviceFields, deviceSettingsOf } from "@maxhq/shared/settings/device-settings";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

/** What a row says when `device`'s `maxd` doesn't know its setting. */
const tooOld = (view: ViewState, device: DeviceId): string =>
	translate("{{name}}'s maxd is too old for this. Update it first.", {
		name: deviceName(view, device),
	});

/**
 * One of `device`'s settings that saves as you pick it, from `known`, what
 * the device has now, or null when its `maxd` is too old for it. Gives what
 * to show, which is your pick until the view moves on from what it had
 * then, the note beside it, and the save.
 */
const useSetting = <K extends keyof DeviceFields>(
	device: DeviceId,
	field: K,
	known: DeviceFields[K] | null
) => {
	const { source } = useServices();
	const view = useView((s) => s.view);
	const [pending, setPending] = useState<{
		value: DeviceFields[K];
		from: DeviceFields[K] | null;
	} | null>(null);
	const [note, setNote] = useState<Note | null>(null);
	const shown = pending && pending.from === known ? pending.value : known;
	const save = async (value: DeviceFields[K]) => {
		setPending({ value, from: known });
		setNote({ text: translate("Saving"), error: false });
		try {
			const failure = failureOf(
				await source.setDeviceSettings({
					device,
					...deviceSettingsOf(view, device),
					[field]: value,
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
	return { shown, note, save };
};

/** The Changes section, with the switch for `device`'s dispatcher. */
export const ActsDirectlySection = ({ device }: { device: DeviceId }) => {
	useI18n();
	const view = useView((s) => s.view);
	const known = actsDirectly(view, device);
	const { shown, note, save } = useSetting(device, "dispatcher_acts", known);
	return (
		<Section title={translate("Changes")}>
			<Row
				title={translate("Act directly")}
				sub={
					known === null
						? tooOld(view, device)
						: translate(
								"The dispatcher changes settings, agents and tasks without asking first. Deleting, stopping busy agents and granting permissions still ask."
							)
				}
			>
				<StatusNote note={note} />
				<Switch
					aria-label={translate("Act directly")}
					checked={shown ?? false}
					disabled={known === null}
					onCheckedChange={(value) => void save(value)}
				/>
			</Row>
		</Section>
	);
};

/** The Chats section, with the pause after which `device`'s dispatcher starts a fresh chat. */
export const ChatGapSection = ({ device }: { device: DeviceId }) => {
	useI18n();
	const view = useView((s) => s.view);
	const known = chatGap(view, device);
	const { shown, note, save } = useSetting(device, "chat_gap_minutes", known);
	const minutes = shown ?? DEFAULT_CHAT_GAP_MINUTES;
	return (
		<Section title={translate("Chats")} hint={translate("30 minutes by default")}>
			<Row
				title={translate("New chat after a pause")}
				sub={
					known === null
						? tooOld(view, device)
						: translate(
								"When you write again after this long, the dispatcher starts a fresh chat. It still goes back to an earlier chat on the same topic."
							)
				}
			>
				<StatusNote note={note} />
				<Picker
					label={translate("New chat after a pause")}
					value={String(minutes)}
					options={chatGapOptions(minutes)}
					onChange={(value) => void save(Number(value))}
					disabled={known === null}
				/>
			</Row>
		</Section>
	);
};
