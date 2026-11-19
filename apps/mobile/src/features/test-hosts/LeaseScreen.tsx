// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One lease, opened from a device's leases or its holder's chips: who
// holds it and how long it has, the test machine's screen, taken again
// every 2 seconds while the lease is ready and this screen is in front,
// the commands it last ran, and End lease.

import { useI18n } from "@maxhq/shared/i18n";

import { useIsFocused } from "@react-navigation/native";
import { useEffect, useState } from "react";
import { Image, ScrollView, StyleSheet, Text, View } from "react-native";

import type { TestLease } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import { useNow } from "@maxhq/shared/lib/clock";
import { formatWhen } from "@maxhq/shared/status/format";
import { commandOutcome, LEASE_SCREEN_EVERY, leaseHeld } from "@maxhq/shared/test-hosts/device";
import {
	LEASE_END,
	leaseCommandLine,
	type LeaseScreen as Shot,
	leaseScreenOf,
} from "@maxhq/shared/test-hosts/format";
import { kindLabel } from "@maxhq/shared/test-hosts/hosts";

import type { ScreenProps } from "../../navigation";
import { useAppActive, useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { Buttons, CardBody, Muted, NoteLine, useAction } from "../devices/section";
import { SCREEN_WAITING, screenHeight } from "./screen";

/** The test machine's screen, taken again while it's ready and in sight. */
const Screen = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const focused = useIsFocused();
	const active = useAppActive();
	const now = useNow(LEASE_SCREEN_EVERY);
	const [shot, setShot] = useState<Shot | null>(null);
	const [problem, setProblem] = useState<string | null>(null);
	const [width, setWidth] = useState(0);
	const [ratio, setRatio] = useState<number | null>(null);
	const ready = lease.state === "ready";
	const taking = ready && focused && active;
	useEffect(() => {
		if (!taking) return;
		let gone = false;
		const take = async () => {
			try {
				const result = await source.readLeaseScreen(lease.device, lease.id);
				if (gone) return;
				const next = leaseScreenOf(result);
				if (next) setShot(next);
				setProblem(next ? null : (failureOf(result) ?? t("It sent no screen.")));
			} catch (error) {
				console.warn("Couldn't take the lease's screen", error);
				if (!gone) setProblem(t("Couldn't reach it. Trying again."));
			}
		};
		void take();
		const timer = setInterval(() => void take(), LEASE_SCREEN_EVERY);
		return () => {
			gone = true;
			clearInterval(timer);
		};
	}, [source, lease.device, lease.id, taking, t]);
	return (
		<View style={styles.shotBlock}>
			<View
				style={[
					styles.shotBox,
					shot ? { height: screenHeight(width, ratio) } : { minHeight: SCREEN_WAITING },
				]}
				onLayout={(e) => setWidth(e.nativeEvent.layout.width)}
			>
				{shot ? (
					<Image
						accessibilityLabel={t("{{kind}} screen", { kind: kindLabel(lease.kind) })}
						source={{ uri: shot.src }}
						resizeMode="contain"
						style={styles.shot}
						onLoad={(e) => {
							const { width: w, height: h } = e.nativeEvent.source;
							if (w > 0 && h > 0) setRatio(w / h);
						}}
					/>
				) : (
					<Text style={styles.waiting}>
						{ready
							? t("Taking the screen.")
							: t("The screen shows once the lease is ready.")}
					</Text>
				)}
			</View>
			{problem || shot ? (
				<Text style={problem ? styles.problem : styles.meta}>
					{problem ??
						(shot
							? t("Taken at {{value1}}", { value1: formatWhen(shot.at, now) })
							: "")}
				</Text>
			) : null}
		</View>
	);
};

const Commands = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const now = useNow(30_000);
	const recent = lease.recent ?? [];
	return (
		<Card title={t("Last commands")}>
			{recent.length === 0 ? (
				<CardBody>
					<Muted>{t("No commands yet.")}</Muted>
				</CardBody>
			) : (
				[...recent].reverse().map((command) => (
					<CardBody key={`${command.at}-${command.command.join(" ")}`}>
						<Text style={styles.command} numberOfLines={3}>
							{leaseCommandLine(command)}
						</Text>
						<Text style={styles.meta}>
							{commandOutcome(command)}, {formatWhen(command.at, now)}
						</Text>
					</CardBody>
				))
			)}
		</Card>
	);
};

const Body = ({ lease }: { lease: TestLease }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const { source } = useServices();
	const now = useNow(30_000);
	const { busy, note, run } = useAction();
	const end = () =>
		confirm(
			{ title: t("End lease"), detail: t("Its holder loses the test machine.") },
			t("End lease"),
			() => void run(t("Ending"), () => source.endLease(lease.device, lease.id))
		);
	return (
		<>
			<Text style={styles.held}>{leaseHeld(view, lease, now)}</Text>
			<Screen lease={lease} />
			<Commands lease={lease} />
			<View style={styles.end}>
				<Buttons>
					<Button tone="danger" label={t("End lease")} disabled={busy} onPress={end} />
				</Buttons>
				<NoteLine note={note} />
			</View>
		</>
	);
};

export const LeaseScreen = ({ navigation, route }: ScreenProps<"Lease">) => {
	const { t } = useI18n();

	const found = useView((s) => s.view.tables.test_lease.get(route.params.lease));
	const open = found !== undefined && found.state !== "ended";
	return (
		<View style={styles.screen}>
			<Header
				title={
					found ? t("{{value1}} lease", { value1: kindLabel(found.kind) }) : t("Lease")
				}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
			/>
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content}>
				{open ? (
					<Body lease={found} />
				) : (
					<Text style={styles.held}>
						{t("This lease has ended.{{value2}}", {
							value2: found?.ended ? ` ${LEASE_END[found.ended]}` : "",
						})}
					</Text>
				)}
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 12 },
	held: { color: color.secondaryForeground, fontSize: size.small, paddingHorizontal: 2 },
	shotBlock: { gap: 6 },
	shotBox: {
		alignItems: "center",
		justifyContent: "center",
		overflow: "hidden",
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	shot: { width: "100%", height: "100%" },
	waiting: {
		paddingHorizontal: 16,
		textAlign: "center",
		color: color.mutedForeground,
		fontSize: size.meta,
	},
	meta: { color: color.mutedForeground, fontSize: size.tiny },
	problem: { color: color.errorText, fontSize: size.tiny },
	command: { color: color.terminal, fontFamily: font.mono, fontSize: size.meta },
	end: { gap: 6, alignItems: "flex-start" },
});
