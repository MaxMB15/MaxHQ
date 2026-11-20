// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A webhook's URL, shown once with Copy, and how a device's webhooks are
// reached: only on the device, over Tailscale Funnel, or at an address of
// your own such as a tunnel. Funnel asks first, since maxd then changes the
// device's Funnel setup, and on Funnel the note says what's missing when it
// doesn't work. Changing the address needs the Mac's switch for this phone,
// so it stays in view, off, until the Mac turns it on.

import { useI18n } from "@maxhq/shared/i18n";

import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceTasks, HookAddress } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { ADDRESS_LABEL, addressNote, addressOf, ownAddressHint } from "@maxhq/shared/tasks/format";

import { useServices } from "../../services";
import { color, font, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
import { choose, confirm } from "../../ui/choose";
import { CopyButton } from "../../ui/CodeCard";
import { Input } from "../../ui/Field";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { Hint, Mono, Notice } from "./fields";

/** A webhook URL, shown only now, with Copy. */
export const HookUrl = ({ url }: { url: string }) => {
	const { t } = useI18n();
	return (
		<Notice title={t("Webhook URL")}>
			<View style={styles.line}>
				<Mono lines={3}>{url}</Mono>
				<CopyButton text={url} />
			</View>
			<Hint>{t("Copy it now. MaxHQ shows it only once, and Rotate makes a new one.")}</Hint>
		</Notice>
	);
};

/** Who reaches a device's webhooks, and on Funnel what's missing when it doesn't work. */
export const AddressNote = ({
	reach,
	device,
}: {
	reach: Pick<DeviceTasks, "address" | "port" | "funnel">;
	device: string;
}) => {
	const { t } = useI18n();
	const note = addressNote(reach, device);
	if (note !== null) return <Hint>{note}</Hint>;
	const { funnel } = reach;
	if (funnel.state !== "needs_setup") return null;
	return (
		<Notice warn title={t("Funnel needs setting up on {{device}}", { device: device })}>
			{funnel.steps.map((step) => (
				<View key={step.reason} style={styles.step}>
					<Text style={styles.reason}>{step.reason}</Text>
					{step.command ? (
						<View style={styles.line}>
							<Mono lines={2}>{step.command}</Mono>
							<CopyButton text={step.command} />
						</View>
					) : null}
				</View>
			))}
		</Notice>
	);
};

// The kinds, whose labels are read as the list opens, in the language then.
const KINDS = Object.keys(ADDRESS_LABEL) as HookAddress["kind"][];

/** A device's webhook address, with what changes it. */
export const AddressRow = ({ reach, name }: { reach: DeviceTasks; name: string }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const allowed = useLetsPhone(reach.device);
	const { funnel } = reach;
	const address = addressOf(reach);
	// The own address being typed, before it's sent.
	const [own, setOwn] = useState<string | null>(null);
	const [failure, setFailure] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const send = async (next: HookAddress) => {
		setFailure(null);
		setBusy(true);
		try {
			const failed = failureOf(await source.setHookAddress(reach.device, next));
			if (failed) return setFailure(failed);
			setOwn(null);
		} catch (error) {
			console.warn("Couldn't change the webhook address", error);
			setFailure(t("Couldn't change the address. Try again."));
		} finally {
			setBusy(false);
		}
	};

	const pick = (kind: HookAddress["kind"]) => {
		setFailure(null);
		setOwn(null);
		if (kind === "funnel")
			confirm(
				{
					title: t("Use Tailscale Funnel on {{device}}?", { device: name }),
					detail: t(
						"maxd runs tailscale funnel on {{device}} to add one path under /maxhq for each webhook that's on, and removes each path when its webhook goes. It leaves the rest of the device's Funnel and Serve setup alone. Anyone with a webhook's URL can reach it from the internet.",
						{ device: name }
					),
				},
				t("Use Funnel"),
				() => void send({ kind: "funnel" })
			);
		else if (kind === "own") setOwn(address.kind === "own" ? address.base : "");
		else if (address.kind !== "local") void send({ kind: "local" });
	};

	const shown = own === null ? address.kind : "own";
	const off = !allowed || busy;
	return (
		<View style={styles.row}>
			<View style={styles.head}>
				<Text style={styles.title}>{t("Webhook address")}</Text>
				<Button
					small
					label={ADDRESS_LABEL[shown]}
					hint={t("Reached")}
					disabled={off}
					onPress={() =>
						choose(
							t("Reached"),
							KINDS.map((kind) => ({
								label: ADDRESS_LABEL[kind],
								disabled: kind === shown,
								pick: () => pick(kind),
							}))
						)
					}
				/>
			</View>
			{own === null ? (
				<AddressNote reach={reach} device={name} />
			) : (
				<View style={styles.own}>
					<Hint>{ownAddressHint(reach.port)}</Hint>
					<Input
						accessibilityLabel={t("Address")}
						value={own}
						onChangeText={setOwn}
						placeholder="https://hooks.example.com"
						autoCapitalize="none"
						autoCorrect={false}
						spellCheck={false}
						autoComplete="off"
						keyboardType="url"
						returnKeyType="done"
						onSubmitEditing={() =>
							!off &&
							own.trim() !== "" &&
							void send({ kind: "own", base: own.trim() })
						}
						style={styles.mono}
					/>
					<View style={styles.buttons}>
						<Button label={t("Cancel")} small onPress={() => setOwn(null)} />
						<Button
							label={t("Use address")}
							tone="primary"
							small
							disabled={off || own.trim() === ""}
							onPress={() => void send({ kind: "own", base: own.trim() })}
						/>
					</View>
				</View>
			)}
			{address.kind === "funnel" && funnel.state === "needs_setup" && own === null ? (
				<View style={styles.buttons}>
					<Button
						label={t("Try again")}
						small
						disabled={off}
						onPress={() => void send({ kind: "funnel" })}
					/>
				</View>
			) : null}
			{failure ? (
				<Text style={styles.failed} accessibilityRole="alert">
					{failure}
				</Text>
			) : null}
			{allowed ? null : <NeedsSwitch />}
		</View>
	);
};

const styles = StyleSheet.create({
	line: { flexDirection: "row", alignItems: "flex-start", gap: 8 },
	step: { gap: 6 },
	reason: { color: color.secondaryForeground, fontSize: size.meta, lineHeight: 17 },
	row: {
		gap: 8,
		paddingHorizontal: 14,
		paddingVertical: 10,
		borderTopColor: color.border,
		borderTopWidth: StyleSheet.hairlineWidth,
	},
	head: { minHeight: TOUCH - 8, flexDirection: "row", alignItems: "center", gap: 12 },
	title: { flex: 1, color: color.foreground, fontSize: size.small },
	own: { gap: 8 },
	mono: { fontFamily: font.mono, fontSize: size.meta },
	buttons: { flexDirection: "row", justifyContent: "flex-end", gap: 8 },
	failed: { color: color.errorText, fontSize: size.meta },
});
