// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which device a link reaches, as the desktop shows it. A link whose hello
// names a device another link already knows waits here for your answer;
// one you said is a different device keeps a note and a way to ask again;
// and a link that reaches its device can forget it, so MaxHQ asks again.
// Both links' details come only to the Mac, so the phone's card leaves
// them out and says what's at stake in words.

import { useI18n } from "@maxhq/shared/i18n";

import { StyleSheet, Text, View } from "react-native";

import type { DeviceLink, Timestamp } from "@maxhq/shared/bindings";
import {
	claimCard,
	claimNames,
	differentNote,
	FORGET_IDENTITY,
	forgetNote,
} from "@maxhq/shared/ui/link-claim";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useServices } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { Buttons, CardBody, Muted, NoteLine, useAction } from "./section";

/** The question a held link waits on, or nothing when it isn't waiting. */
export const LinkClaimCard = ({
	link,
	view,
	now,
}: {
	link: DeviceLink;
	view: ViewState;
	now: Timestamp;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const { busy, note, run } = useAction();
	const names = claimNames(view, link);
	if (!names || link.claim?.state !== "asking") return null;
	const card = claimCard(names, null, now, t("Identity, on this device's page"));
	return (
		<View style={styles.claim} accessibilityLabel={card.title}>
			<Text accessibilityRole="header" style={styles.title}>
				{card.title}
			</Text>
			<Text style={styles.risk}>{card.risk}</Text>
			<Text style={styles.remember}>{card.remember}</Text>
			<Buttons>
				<Button
					label={card.different}
					grow={1}
					disabled={busy}
					onPress={() => void run(null, () => source.answerLinkClaim(link.id, false))}
				/>
				<Button
					label={card.same}
					tone="primary"
					grow={1}
					disabled={busy}
					onPress={() => void run(null, () => source.answerLinkClaim(link.id, true))}
				/>
			</Buttons>
			<NoteLine note={note} />
		</View>
	);
};

/** Which device an SSH link reaches, with a way to forget it or ask again. */
export const DeviceIdentity = ({ link, view }: { link: DeviceLink; view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAction();
	if (link.route.type !== "ssh") return null;
	const names = claimNames(view, link);
	if (names && !names.different) return null;
	const forget = (
		<Button
			label={t(FORGET_IDENTITY)}
			tone="danger"
			small
			disabled={busy}
			onPress={() =>
				confirm(
					{ title: t(FORGET_IDENTITY), detail: t("MaxHQ asks again at its next hello.") },
					t("Forget"),
					() => void run(null, () => source.forgetLinkIdentity(link.id))
				)
			}
		/>
	);
	return (
		<Card title={t("Identity")} label={t("Identity of {{name}}", { name: link.name })}>
			<CardBody>
				{names ? (
					<>
						<Text style={styles.line}>{differentNote(names)}</Text>
						<Buttons>
							<Button
								label={t("Ask again")}
								small
								disabled={busy}
								onPress={() =>
									void run(null, () => source.askLinkClaimAgain(link.id))
								}
							/>
							{forget}
						</Buttons>
					</>
				) : link.device ? (
					<>
						<Text style={styles.line}>
							{forgetNote(
								link.name,
								view.tables.device.get(link.device)?.name ?? link.name
							)}
						</Text>
						<Buttons>{forget}</Buttons>
					</>
				) : (
					<>
						<Text style={styles.line}>
							{t("{{name}} hasn't named its device yet", { name: link.name })}
						</Text>
						<Muted>{t("MaxHQ learns it at the link's first hello.")}</Muted>
					</>
				)}
				<NoteLine note={note} />
			</CardBody>
		</Card>
	);
};

const styles = StyleSheet.create({
	claim: {
		gap: 8,
		padding: 12,
		borderRadius: radius.lg,
		borderWidth: 1,
		borderColor: tint(color.busy, 0.5),
		backgroundColor: tint(color.busy, 0.08),
	},
	title: { color: color.foregroundStrong, fontSize: size.body, fontWeight: "700" },
	risk: { color: color.foreground, fontSize: size.small, lineHeight: 19 },
	remember: { color: color.mutedForeground, fontSize: size.meta },
	line: { color: color.foreground, fontSize: size.small },
});
