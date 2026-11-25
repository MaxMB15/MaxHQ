// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One device's page, opened from the Devices list, as the desktop's main
// area shows it: how the phone reaches it, the chain of checks to its
// agents with the one that stopped marked, its load, setup, workspaces,
// CLIs, tools and test hosts, which device it is, winding it down for a
// restart, and the agents it runs or last ran, the settled ones last. The
// page acts through the device's own link. What only the Mac can do, such
// as restarting maxd or opening Terminal, says so or stays off the page.

import { formatNumber, useI18n } from "@maxhq/shared/i18n";

import { memo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Svg, { Circle, Path } from "react-native-svg";

import type { Agent, DeviceId, Timestamp } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { ACTIVITY_LABEL, agentLook } from "@maxhq/shared/status/activity";
import type { Check, CheckState } from "@maxhq/shared/status/card";
import { deviceConnection, linkChecks } from "@maxhq/shared/status/connection";
import {
	DEFAULT_WIND_DOWN_MINUTES,
	deviceChips,
	splitDetail,
	WIND_DOWN_MINUTES,
} from "@maxhq/shared/status/device-page";
import { formatDuration, formatWhen } from "@maxhq/shared/status/format";
import { versionNote } from "@maxhq/shared/status/reach";
import { accountName, VENDOR_NAME } from "@maxhq/shared/status/usage";
import { leasesOf } from "@maxhq/shared/test-hosts/hosts";
import { openAdmissions } from "@maxhq/shared/ui/admission";
import { inboxOf } from "@maxhq/shared/view/inbox";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceNode } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, radius, size, tint } from "../../theme";
import { Button } from "../../ui/Button";
import { Card, CardRow } from "../../ui/Card";
import { choose } from "../../ui/choose";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { ActivityDot, ConnectionDot, Spinner } from "../../ui/Dot";
import { Header } from "../../ui/Header";
import { AdmissionCard } from "../attention/AdmissionCard";
import { DeviceTestHosts } from "../test-hosts/DeviceTestHosts";
import { LeaseChips } from "../test-hosts/LeaseChips";
import { DeviceMoves } from "../workspaces/DeviceMoves";
import { DeviceWorkspaces } from "../workspaces/DeviceWorkspaces";
import { DeviceIdentity, LinkClaimCard } from "./DeviceIdentity";
import { DeviceInstalls } from "./DeviceInstalls";
import { DeviceLoad } from "./DeviceLoad";
import { DeviceRegistry } from "./DeviceRegistry";
import { DeviceSetup } from "./DeviceSetup";
import { DeviceTools } from "./DeviceTools";
import { Buttons, CardBody, CopyTextButton, Muted, NoteLine, useAction } from "./section";

const ICON = 14;

const CheckIcon = ({ state }: { state: CheckState }) => {
	const { t } = useI18n();

	switch (state) {
		case "passed":
			return (
				<View
					accessible
					accessibilityRole="image"
					accessibilityLabel={t("Passed")}
					style={[styles.icon, { backgroundColor: tint(color.ready, 0.18) }]}
				>
					<Svg width={9} height={9} viewBox="0 0 24 24" fill="none">
						<Path
							d="M5 12l5 5 9-10"
							stroke={color.ready}
							strokeWidth={3.6}
							strokeLinecap="round"
							strokeLinejoin="round"
						/>
					</Svg>
				</View>
			);
		case "running":
			return (
				<View
					accessible
					accessibilityRole="image"
					accessibilityLabel={t("Running")}
					style={[styles.icon, { backgroundColor: tint(color.connecting, 0.2) }]}
				>
					<Spinner tone={color.connecting} size={10} width={1.5} />
				</View>
			);
		case "failed":
			return (
				<View
					accessible
					accessibilityRole="image"
					accessibilityLabel={t("Failed")}
					style={[styles.icon, { backgroundColor: tint(color.error, 0.2) }]}
				>
					<Svg width={8} height={8} viewBox="0 0 24 24" fill="none">
						<Path
							d="M6 6l12 12M18 6L6 18"
							stroke={color.error}
							strokeWidth={4}
							strokeLinecap="round"
						/>
					</Svg>
				</View>
			);
		case "not_run":
			return (
				<View
					accessible
					accessibilityRole="image"
					accessibilityLabel={t("Not run")}
					style={[styles.icon, styles.notRun]}
				/>
			);
		case "unknown":
			return (
				<View
					accessible
					accessibilityRole="image"
					accessibilityLabel={t("Unknown")}
					style={[styles.icon, styles.unknown]}
				>
					<Text style={styles.question}>?</Text>
				</View>
			);
	}
};

const CheckRow = memo(function CheckRow({ check, first }: { check: Check; first: boolean }) {
	const failed = check.state === "failed";
	const [main, sub] = failed ? splitDetail(check.detail) : [check.detail, undefined];
	return (
		<View style={[styles.check, !first && styles.divider, failed && styles.failedCheck]}>
			<CheckIcon state={check.state} />
			<View style={styles.checkText}>
				<Text
					style={[
						styles.checkTitle,
						failed && styles.checkTitleFailed,
						check.state === "not_run" && styles.muted,
					]}
				>
					{check.title}
				</Text>
				{main ? (
					<Text style={[styles.checkDetail, failed && styles.errorText]}>{main}</Text>
				) : null}
				{sub ? <Text style={styles.checkSub}>{sub}</Text> : null}
			</View>
		</View>
	);
});

/** The banner while a device winds down for a restart, and after. */
const WindingDown = ({ name, until }: { name: string; until: Timestamp }) => {
	const { t } = useI18n();

	const now = useNow(1000);
	const ahead = until > now;
	return (
		<View
			accessibilityRole="summary"
			accessibilityLabel={t("Winding down")}
			accessibilityLiveRegion="polite"
			style={styles.winding}
		>
			<View style={styles.windingDot}>
				<ConnectionDot look="connecting" size={10} surface={color.background} decorative />
			</View>
			<View style={styles.windingText}>
				<Text style={styles.windingTitle}>
					{ahead
						? t("{{name}} is winding down for a restart", { name: name })
						: t("{{name}} has wound down for a restart", { name: name })}
				</Text>
				<Text style={styles.windingBody}>
					{ahead
						? t(
								"Busy agents are finishing the step they're on. The window closes at {{value1}}, in {{value2}}.",
								{
									value1: formatWhen(until, now),
									value2: formatDuration(until - now),
								}
							)
						: t(
								"The window closed at {{value1}}. If {{name}} drops for its restart, MaxHQ tries it every 2 seconds for 10 minutes, and its agents resume once it's back.",
								{ value1: formatWhen(until, now), name: name }
							)}
				</Text>
			</View>
		</View>
	);
};

/** Wind down, and a note that restarting maxd is the Mac's. */
const Restart = ({
	device,
	name,
	live,
	winding,
}: {
	device: DeviceId | null;
	name: string;
	live: boolean;
	/** Whether a window is already open. */
	winding: boolean;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [minutes, setMinutes] = useState(DEFAULT_WIND_DOWN_MINUTES);
	const { busy, note, run } = useAction();
	const label = (m: number) => t("{{m}} min", { m: formatNumber(m) });
	const pick = () =>
		choose(
			t("Window"),
			WIND_DOWN_MINUTES.map((m) => ({
				label: label(m),
				disabled: m === minutes,
				pick: () => setMinutes(m),
			}))
		);
	return (
		<Card
			title={t("Restart")}
			note={t("For a restart you plan, such as an update")}
			label={t("Restart {{name}}", { name: name })}
		>
			{device ? (
				<>
					<CardBody>
						<Text style={styles.rowTitle}>{t("Wind down first")}</Text>
						<Muted>
							{live
								? t(
										"Asks the busy agents MaxHQ runs on {{name}} to finish the step they're on and start no long command. Restart {{name2}} once the window closes, and they pick up after it.",
										{ name: name, name2: name }
									)
								: t("Connect to {{name}} to wind it down.", { name: name })}
						</Muted>
					</CardBody>
					<CardRow label={t("Window")} onPress={live && !busy ? pick : undefined}>
						<Text style={styles.value}>{label(minutes)}</Text>
					</CardRow>
					<CardBody>
						<Buttons>
							<Button
								label={t("Wind down for restart")}
								small
								disabled={!live || busy || winding}
								onPress={() =>
									void run(t("Sending"), () =>
										source.windDown(device, minutes * 60)
									)
								}
							/>
						</Buttons>
						<NoteLine note={note} />
					</CardBody>
				</>
			) : null}
			<CardRow label={t("Restart maxd")}>
				<Text style={styles.value}>{t("On the Mac")}</Text>
			</CardRow>
		</Card>
	);
};

const AgentLine = memo(function AgentLine({
	agent,
	view,
	now,
	live,
	first,
	open,
}: {
	agent: Agent;
	view: ViewState;
	now: Timestamp;
	live: boolean;
	first: boolean;
	open: (agent: Agent) => void;
}) {
	const { t } = useI18n();

	const workspace = view.tables.workspace.get(agent.workspace);
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const where = [
		workspace?.location.type === "container" ? workspace.location.instance : undefined,
		account ? accountName(account, view.tables.model_server) : VENDOR_NAME[agent.vendor],
	]
		.filter(Boolean)
		.join(", ");
	const look = agentLook(agent, now);
	const settled = inboxOf(agent).settled;
	const state = settled
		? t("Settled {{value1}}", { value1: formatWhen(settled.at, now) })
		: live
			? ACTIVITY_LABEL[look]
			: t("Last seen {{value1}}", { value1: formatWhen(agent.last_activity_at, now) });
	return (
		<View style={[!first && styles.divider]}>
			<Pressable
				accessibilityRole="link"
				accessibilityLabel={`${agent.title}. ${where}. ${state}`}
				onPress={() => open(agent)}
				style={({ pressed }) => [styles.agent, pressed && styles.pressed]}
			>
				{settled ? (
					<Svg width={12} height={12} viewBox="0 0 24 24" fill="none">
						<Circle
							cx={12}
							cy={12}
							r={10}
							stroke={color.mutedForeground}
							strokeWidth={2}
						/>
						<Path
							d="M8 12l3 3 5-6"
							stroke={color.mutedForeground}
							strokeWidth={2}
							strokeLinecap="round"
							strokeLinejoin="round"
						/>
					</Svg>
				) : (
					<ActivityDot look={look} size={10} surface={color.card} decorative />
				)}
				<View style={styles.agentText}>
					<Text numberOfLines={1} style={styles.agentTitle}>
						{agent.title}
					</Text>
					<Text numberOfLines={1} style={styles.agentMeta}>
						{where ? `${where} · ${state}` : state}
					</Text>
				</View>
			</Pressable>
			{leasesOf(view, { kind: "agent", agent: agent.id }).length > 0 ? (
				<View style={styles.chips}>
					<LeaseChips agent={agent.id} />
				</View>
			) : null}
		</View>
	);
});

const Agents = ({
	agents,
	settled,
	name,
	view,
	now,
	live,
	open,
}: {
	agents: readonly Agent[];
	settled: readonly Agent[];
	name: string;
	view: ViewState;
	now: Timestamp;
	live: boolean;
	open: (agent: Agent) => void;
}) => {
	const { t } = useI18n();
	return (
		<>
			<Card
				title={live ? t("Agents here") : t("Agents last seen here")}
				label={t("Agents on {{name}}", { name: name })}
			>
				{agents.length === 0 ? (
					<CardBody>
						<Muted>
							{settled.length > 0
								? t("Every agent on {{name}} is settled.", { name: name })
								: t("No agents on {{name}}.", { name: name })}
						</Muted>
					</CardBody>
				) : (
					<View style={styles.divider}>
						{agents.map((agent, i) => (
							<AgentLine
								key={agent.id}
								agent={agent}
								view={view}
								now={now}
								live={live}
								first={i === 0}
								open={open}
							/>
						))}
					</View>
				)}
				{!live && agents.length > 0 ? (
					<CardBody>
						<Muted>
							{t(
								"These are the agents MaxHQ saw before it lost touch with {{name}}. Their state updates once it answers again.",
								{ name: name }
							)}
						</Muted>
					</CardBody>
				) : null}
			</Card>
			{settled.length > 0 ? (
				<Card title={t("Settled")}>
					<View style={styles.divider}>
						{settled.map((agent, i) => (
							<AgentLine
								key={agent.id}
								agent={agent}
								view={view}
								now={now}
								live={live}
								first={i === 0}
								open={open}
							/>
						))}
					</View>
				</Card>
			) : null}
		</>
	);
};

export const DeviceScreen = ({ navigation, route }: ScreenProps<"Device">) => {
	const { t } = useI18n();

	const { source } = useServices();
	const view = useView((s) => s.view);
	const now = useNow(5000);
	const link = view.tables.device_link.get(route.params.link);
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
	const node = deviceNode(view, link, now);
	const connection = deviceConnection(node.ctx, now);
	const checks = linkChecks(node.ctx, now);
	const live = connection.look === "live";
	const until = node.ctx.device?.winding_down_until ?? null;
	const version = versionNote(link);
	const tone =
		connection.look === "unreachable"
			? color.errorText
			: connection.look === "connecting"
				? color.connectingText
				: connection.look === "agent_down"
					? color.linked
					: color.secondaryForeground;
	const device = link.device;
	const admissions = device ? openAdmissions(view, device) : [];
	const open = (agent: Agent) => navigation.navigate("Agent", { agent: agent.id });
	const section = device ? { device, name: link.name, live, view } : null;

	return (
		<View style={styles.screen}>
			<Header title={link.name} back={back} />
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content}>
				<View style={styles.top}>
					<Text style={[styles.status, { color: tone }]}>
						<Text style={styles.statusTitle}>{connection.card.title}.</Text>{" "}
						{connection.card.body}
					</Text>
					<View style={styles.chipRow}>
						{deviceChips(node).map((chip) => (
							<View key={chip} style={styles.chip}>
								<Text style={styles.chipText}>{chip}</Text>
							</View>
						))}
					</View>
					{version ? <Text style={styles.version}>{version}</Text> : null}
					<Buttons>
						<Button
							label={t("Try now")}
							tone="primary"
							small
							onPress={() => void source.refreshLinks(link.id)}
						/>
						{link.route.type === "ssh" ? (
							<CopyTextButton
								label={t("Copy SSH command")}
								text={`ssh ${link.route.host}`}
							/>
						) : null}
					</Buttons>
				</View>

				{until !== null ? <WindingDown name={link.name} until={until} /> : null}

				<LinkClaimCard link={link} view={view} now={now} />
				{admissions.map((admission) => (
					<AdmissionCard key={admission.id} admission={admission} view={view} />
				))}

				<Card
					title={t("Where the connection stops")}
					note={t("Each check runs only if the one above passed")}
					label={t("Connection checks")}
				>
					<View style={styles.divider}>
						{checks.map((check, i) => (
							<CheckRow key={check.id} check={check} first={i === 0} />
						))}
					</View>
					{!live ? (
						<View style={[styles.footer, styles.divider]}>
							<Text style={styles.footerText}>
								{t("Try now runs every check again from the top.")}
							</Text>
						</View>
					) : null}
				</Card>

				{section ? (
					<>
						<DeviceLoad {...section} />
						<DeviceSetup {...section} />
						<DeviceWorkspaces {...section} />
						<DeviceMoves {...section} />
						<DeviceTools {...section} />
						<DeviceInstalls {...section} />
						<DeviceRegistry {...section} />
						<DeviceTestHosts {...section} />
						<Card>
							<CardRow
								first
								label={t("Device settings")}
								onPress={() =>
									navigation.navigate("DeviceSettings", {
										device: section.device,
									})
								}
							/>
						</Card>
					</>
				) : null}
				<DeviceIdentity link={link} view={view} />

				<Restart
					device={device}
					name={link.name}
					live={live}
					winding={until !== null && until > now}
				/>

				<Agents
					agents={node.agents}
					settled={node.settled}
					name={link.name}
					view={view}
					now={now}
					live={live}
					open={open}
				/>
			</ScrollView>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	top: { gap: 8 },
	status: { fontSize: size.small, lineHeight: 19 },
	statusTitle: { fontWeight: "600" },
	chipRow: { flexDirection: "row", flexWrap: "wrap", gap: 6 },
	chip: {
		height: 20,
		justifyContent: "center",
		borderRadius: 5,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.secondary,
		paddingHorizontal: 7,
	},
	chipText: { color: color.secondaryForeground, fontSize: size.tiny },
	version: { color: color.mutedForeground, fontSize: size.meta },
	divider: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	check: {
		flexDirection: "row",
		alignItems: "flex-start",
		gap: 12,
		paddingHorizontal: 14,
		paddingVertical: 10,
	},
	failedCheck: { backgroundColor: tint(color.error, 0.06) },
	icon: {
		width: ICON,
		height: ICON,
		borderRadius: ICON / 2,
		alignItems: "center",
		justifyContent: "center",
		marginTop: 2,
	},
	notRun: { borderWidth: 1.5, borderStyle: "dashed", borderColor: color.off },
	unknown: { borderWidth: 1.5, borderColor: color.off },
	question: { color: color.mutedForeground, fontSize: 9, fontWeight: "700", lineHeight: 11 },
	checkText: { flex: 1, gap: 2 },
	checkTitle: { color: color.foreground, fontSize: size.small },
	checkTitleFailed: { color: color.foregroundStrong, fontWeight: "600" },
	muted: { color: color.mutedForeground },
	checkDetail: { color: color.mutedForeground, fontSize: size.meta },
	errorText: { color: color.errorText },
	checkSub: { color: color.mutedForeground, fontSize: size.tiny },
	footer: { paddingHorizontal: 14, paddingVertical: 10, backgroundColor: color.subpanel },
	footerText: { color: color.mutedForeground, fontSize: size.meta },
	winding: {
		flexDirection: "row",
		gap: 12,
		borderRadius: radius.lg,
		borderWidth: 1,
		borderColor: tint(color.connecting, 0.4),
		backgroundColor: tint(color.connecting, 0.1),
		paddingHorizontal: 14,
		paddingVertical: 12,
	},
	windingDot: { marginTop: 3 },
	windingText: { flex: 1, gap: 3 },
	windingTitle: { color: color.connectingText, fontSize: size.small, fontWeight: "700" },
	windingBody: { color: color.secondaryForeground, fontSize: size.meta, lineHeight: 17 },
	rowTitle: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	value: { color: color.mutedForeground, fontSize: size.small },
	agent: {
		minHeight: 44,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 14,
		paddingVertical: 8,
	},
	pressed: { backgroundColor: color.accent },
	agentText: { flex: 1, gap: 1 },
	agentTitle: { color: color.terminal, fontSize: size.small },
	agentMeta: { color: color.mutedForeground, fontSize: size.meta },
	chips: { paddingLeft: 34, paddingRight: 14, paddingBottom: 10 },
});
