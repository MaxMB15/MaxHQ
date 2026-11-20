// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// An agent's ports and processes, as the desktop's workspace popover lists
// them: where the agent runs, the ports this phone forwards with Open and
// Stop forwarding, the agents' other ports with Forward, the ports no agent
// opened, then what the agent started and still runs, with Stop and Kill
// after a confirm. A forward listens on the phone itself, so Open shows its
// page here. Forward and Stop need Let this phone do everything on the Mac
// and say so while it's off. Anyone can look. A pull reads the processes
// again, as they're read every few seconds while the app is in front.

import { formatNumber, t, useI18n } from "@maxhq/shared/i18n";

import Unplug from "lucide-react-native/icons/unplug";
import { Fragment, type ReactNode, useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import type { AgentId, AgentProcess, ListeningPort } from "@maxhq/shared/bindings";
import { formatMemory } from "@maxhq/shared/fleet/memory";
import { useNow } from "@maxhq/shared/lib/clock";
import { type PortActions, usePortActions } from "@maxhq/shared/ports/actions";
import {
	formatCpu,
	type ProcessGroup,
	processKey,
	runningList,
	shortCommand,
	stopOrder,
	turnEnded,
} from "@maxhq/shared/ports/processes";
import { type ProcessRead, type Stops, useProcesses, useStops } from "@maxhq/shared/ports/running";
import {
	forwardsPorts,
	isRemoteDevice,
	isUnreachable,
	placeName,
} from "@maxhq/shared/ports/select";
import { formatDuration } from "@maxhq/shared/status/format";
import { all } from "@maxhq/shared/view/replica";
import { deviceName, linkForDevice } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useAppActive, useServices, useView } from "../../services";
import { color, font, radius, size } from "../../theme";
import { Button } from "../../ui/Button";
import { confirm } from "../../ui/choose";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Spinner } from "../../ui/Dot";
import { ChevronDown, ChevronRight } from "../../ui/glyphs";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { PortPage } from "./PortPage";
import {
	PHONE_HOOKS,
	type PhoneForward,
	portSections,
	useMyForwards,
	watchMyForwards,
} from "./running";

const plain = (n: number) => formatNumber(n, { useGrouping: false });

/** Why a row's last action failed, under it. */
const Failure = ({ text }: { text: string | null }) =>
	text === null ? null : (
		<Text accessibilityLiveRegion="polite" style={styles.failure}>
			{text}
		</Text>
	);

/** A titled list, as the popover's sections. */
const Section = ({ title, children }: { title: string; children: ReactNode }) => (
	<View style={styles.section}>
		<Text accessibilityRole="header" style={styles.sectionTitle}>
			{title}
		</Text>
		<View style={styles.card}>{children}</View>
	</View>
);

/** A port's line: its number, the process holding it, the row's buttons and its notes. */
const PortLine = ({
	port,
	first,
	quiet,
	notes,
	children,
}: {
	port: ListeningPort;
	first: boolean;
	quiet?: boolean;
	notes?: ReactNode;
	children?: ReactNode;
}) => {
	const { t } = useI18n();
	return (
		<View style={[styles.row, !first && styles.line]}>
			<View style={styles.rowTop}>
				<Text style={styles.portNumber}>{plain(port.port)}</Text>
				<View style={styles.grow}>
					<Text numberOfLines={1} style={[styles.name, quiet && styles.quiet]}>
						{port.process?.name ?? t("Unknown process")}
					</Text>
					{port.process ? (
						<Text numberOfLines={1} style={styles.command}>
							{port.process.command}
						</Text>
					) : null}
				</View>
				{children}
			</View>
			{notes}
		</View>
	);
};

const ForwardedRow = ({
	port,
	forward,
	first,
	actions,
	onOpen,
}: {
	port: ListeningPort;
	forward: PhoneForward;
	first: boolean;
	actions: PortActions;
	onOpen: (local: number) => void;
}) => {
	const { t } = useI18n();

	const id = forward.forward;
	const stopping = actions.busy(port.id);
	return (
		<PortLine
			port={port}
			first={first}
			notes={
				<>
					{forward.local !== port.port ? (
						<Text style={styles.note}>
							{t("{{port}} is busy on this phone, forwarded to {{local_port}}", {
								port: plain(port.port),
								local_port: plain(forward.local),
							})}
						</Text>
					) : null}
					<Failure text={actions.failure(port.id)} />
				</>
			}
		>
			<Button label={t("Open")} small onPress={() => onOpen(forward.local)} />
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={t("Stop forwarding {{port}}", { port: plain(port.port) })}
				accessibilityState={{ disabled: stopping || id === null, busy: stopping }}
				disabled={stopping || id === null}
				hitSlop={6}
				onPress={() => id !== null && actions.stop(port, id)}
				style={({ pressed }) => [
					styles.iconButton,
					pressed && styles.pressed,
					(stopping || id === null) && styles.off,
				]}
			>
				{stopping ? (
					<Spinner size={12} tone={color.mutedForeground} />
				) : (
					<Unplug size={15} strokeWidth={2.1} color={color.secondaryForeground} />
				)}
			</Pressable>
		</PortLine>
	);
};

const ListeningRow = ({
	port,
	first,
	quiet,
	forwards,
	lets,
	actions,
}: {
	port: ListeningPort;
	first: boolean;
	quiet?: boolean;
	/** Whether the device takes a forward now. */
	forwards: boolean;
	/** Whether the device takes this phone's forwards. */
	lets: boolean;
	actions: PortActions;
}) => {
	const { t } = useI18n();

	const busy = actions.busy(port.id);
	return (
		<PortLine
			port={port}
			first={first}
			quiet={quiet}
			notes={<Failure text={actions.failure(port.id)} />}
		>
			{forwards ? (
				<Button
					label={t("Forward")}
					hint={t("Forward port {{port}}", { port: plain(port.port) })}
					small
					disabled={busy || !lets}
					icon={busy ? <Spinner size={10} tone={color.mutedForeground} /> : undefined}
					onPress={() => actions.forward(port)}
				/>
			) : null}
		</PortLine>
	);
};

/** Asks before Stop or Kill reaches `processes`, in the order they get it. */
const askToStop = (
	stops: Stops,
	key: string,
	name: string,
	processes: readonly AgentProcess[],
	kill: boolean
) =>
	kill
		? confirm(
				{
					title: t("Kill {{name}}", { name }),
					detail: t("Still running after Stop. Kill ends it at once."),
				},
				t("Kill"),
				() => stops.signal(key, processes, "kill")
			)
		: confirm(
				{
					title: t("Stop {{name}}", { name }),
					detail: t(
						"This asks it and everything under it to end. If it keeps running, Kill ends it at once."
					),
				},
				t("Stop"),
				() => stops.signal(key, processes, "term")
			);

/**
 * One line of the Running list: a process, or a command with the totals of
 * everything under it and a chevron that unfolds them.
 */
const ProcessRow = ({
	process,
	first,
	inset,
	cpu,
	after,
	stops,
	lets,
	group,
}: {
	process: AgentProcess;
	first: boolean;
	/** How far in it sits, which shows where in a tree it is. */
	inset: number;
	cpu: number;
	/** Whether to mark it as running after the agent's turn, unless it only keeps the machine awake. */
	after: boolean;
	/** Stop and Kill, for what the agent started. */
	stops?: Stops;
	lets: boolean;
	group?: { of: ProcessGroup; open: boolean; toggle: () => void };
}) => {
	const { t } = useI18n();

	const name = shortCommand(process);
	const key = processKey(process);
	const under = group?.of.under.length ?? 0;
	const ports = group?.of.ports ?? process.ports;
	const state = stops?.state(key);
	const Fold = group?.open ? ChevronDown : ChevronRight;
	const meta = [
		formatDuration(process.run_ms),
		t("{{value1}} CPU", { value1: formatCpu(cpu) }),
		formatMemory(group?.of.memory ?? process.memory),
		under > 0 ? t("{{value1}} processes", { value1: formatNumber(under + 1) }) : null,
	].filter((part): part is string => part !== null);
	const mark = group?.of.keepsAwake ? t("keeps awake") : after ? t("after the turn") : null;
	const markHint = group?.of.keepsAwake
		? t("It only keeps the computer awake and does no work.")
		: t("It's still running after the agent's turn ended.");

	return (
		<View style={[styles.row, { paddingLeft: inset }, !first && styles.line]}>
			<View style={styles.rowTop}>
				{group ? (
					under > 0 ? (
						<Pressable
							accessibilityRole="button"
							accessibilityLabel={t("The {{value1}} processes of {{name}}", {
								value1: formatNumber(under + 1),
								name,
							})}
							accessibilityState={{ expanded: group.open }}
							hitSlop={10}
							onPress={group.toggle}
							style={({ pressed }) => [styles.fold, pressed && styles.pressed]}
						>
							<Fold size={14} strokeWidth={2.2} color={color.mutedForeground} />
						</Pressable>
					) : (
						<View style={styles.fold} />
					)
				) : null}
				<Text numberOfLines={1} style={[styles.processName, styles.grow]}>
					{name}
				</Text>
				{ports.map((port) => (
					<Text
						key={port}
						accessibilityLabel={t("Listening on port {{port}}", { port: plain(port) })}
						style={styles.chip}
					>
						{plain(port)}
					</Text>
				))}
				{stops ? (
					<Button
						label={
							state === "kill"
								? t("Kill")
								: state === "stopping"
									? t("Stopping")
									: t("Stop")
						}
						tone={state === "kill" ? "danger" : "secondary"}
						hint={
							state === "kill"
								? t("Kill {{name}}", { name })
								: t("Stop {{name}}", { name })
						}
						small
						disabled={!lets || state === "stopping"}
						onPress={() =>
							askToStop(
								stops,
								key,
								name,
								group ? stopOrder(group.of) : [process],
								state === "kill"
							)
						}
					/>
				) : null}
			</View>
			<Text style={[styles.meta, group && styles.metaUnder]}>
				{meta.join("  ")}
				{mark !== null ? (
					<Text accessibilityHint={markHint} style={styles.mark}>
						{`  ${mark}`}
					</Text>
				) : null}
			</Text>
			{stops ? <Failure text={stops.failure(key)} /> : null}
		</View>
	);
};

/** What the agent started and still runs, with its own processes in a group below. */
const Processes = ({
	agent,
	read,
	failure,
	stops,
	lets,
	ended,
}: {
	agent: AgentId;
	read: ProcessRead | null;
	failure: string | null;
	stops: Stops;
	lets: boolean;
	ended: boolean;
}) => {
	const { t } = useI18n();

	const [open, setOpen] = useState(false);
	const [unfolded, setUnfolded] = useState<ReadonlySet<string>>(() => new Set());
	const toggle = (key: string) =>
		setUnfolded((was) => {
			const next = new Set(was);
			if (!next.delete(key)) next.add(key);
			return next;
		});

	if (failure !== null) {
		return (
			<Section title={t("Running")}>
				<Text accessibilityRole="alert" style={[styles.empty, styles.error]}>
					{t("Couldn't list the agent's processes: {{message}}", { message: failure })}
				</Text>
			</Section>
		);
	}
	if (!read || read.agent !== agent) {
		return (
			<Section title={t("Running")}>
				<View style={styles.loading}>
					<Spinner size={12} tone={color.mutedForeground} />
					<Text style={styles.muted}>{t("Loading")}</Text>
				</View>
			</Section>
		);
	}
	if (read.list.status === "too_old") {
		return (
			<Section title={t("Running")}>
				<Text style={styles.empty}>
					{t("This device's maxd is too old to list processes.")}
				</Text>
			</Section>
		);
	}
	const { started, own } = runningList(read.list.processes);
	const cpu = (process: AgentProcess) => read.cpu.get(processKey(process)) ?? 0;

	return (
		<Section title={t("Running")}>
			{started.map((group, i) => {
				const key = processKey(group.head);
				const shown = unfolded.has(key);
				return (
					<Fragment key={key}>
						<ProcessRow
							process={group.head}
							first={i === 0}
							inset={8}
							cpu={[group.head, ...group.under].reduce((sum, p) => sum + cpu(p), 0)}
							after={ended}
							stops={stops}
							lets={lets}
							group={{ of: group, open: shown, toggle: () => toggle(key) }}
						/>
						{shown
							? group.under.map((process) => (
									<ProcessRow
										key={processKey(process)}
										process={process}
										first={false}
										inset={36}
										cpu={cpu(process)}
										after={false}
										stops={stops}
										lets={lets}
									/>
								))
							: null}
					</Fragment>
				);
			})}
			{started.length === 0 ? (
				<Text style={styles.empty}>{t("Nothing the agent started is running.")}</Text>
			) : null}
			{own.length > 0 ? (
				<>
					<Pressable
						accessibilityRole="button"
						accessibilityState={{ expanded: open }}
						onPress={() => setOpen((was) => !was)}
						style={({ pressed }) => [
							styles.group,
							styles.line,
							pressed && styles.pressed,
						]}
					>
						{open ? (
							<ChevronDown
								size={13}
								strokeWidth={2.2}
								color={color.mutedForeground}
							/>
						) : (
							<ChevronRight
								size={13}
								strokeWidth={2.2}
								color={color.mutedForeground}
							/>
						)}
						<Text style={styles.groupText}>{t("Agent")}</Text>
						<Text style={styles.groupCount}>{formatNumber(own.length)}</Text>
					</Pressable>
					{open ? (
						<>
							{own.map(({ process, child }) => (
								<ProcessRow
									key={processKey(process)}
									process={process}
									first={false}
									inset={child ? 28 : 14}
									cpu={cpu(process)}
									after={false}
									lets={lets}
								/>
							))}
							<Text style={[styles.empty, styles.line]}>
								{t("These stop with the agent.")}
							</Text>
						</>
					) : null}
				</>
			) : null}
			{!lets && started.length > 0 ? (
				<View style={[styles.switchNote, styles.line]}>
					<NeedsSwitch />
				</View>
			) : null}
		</Section>
	);
};

export const RunningScreen = ({ navigation, route }: ScreenProps<"Running">) => {
	const { t } = useI18n();

	const { source, view: store } = useServices();
	const view = useView((s) => s.view);
	const mine = useMyForwards((s) => s.mine);
	const now = useNow(1000);
	const active = useAppActive();
	const actions = usePortActions(source, PHONE_HOOKS);
	const [page, setPage] = useState<number | null>(null);
	useEffect(() => watchMyForwards(store), [store]);

	const id = route.params.agent;
	const agent = view.tables.agent.get(id);
	const workspace = agent ? view.tables.workspace.get(agent.workspace) : undefined;
	const device = workspace?.device ?? null;
	const lets = useLetsPhone(device);
	const remote = device !== null && isRemoteDevice(view, device);
	const forwards = device !== null && forwardsPorts(view, device);
	const offline = remote && device !== null && isUnreachable(view, device, now);
	const live = agent !== undefined && !offline && active;
	const { read, failure, refresh } = useProcesses(source, id, live);
	const stops = useStops(source, id, refresh);
	const refreshControl = usePullToRefresh(refresh);
	const back = { label: t("Back"), onPress: () => navigation.goBack() };

	if (!agent) {
		return (
			<View style={styles.screen}>
				<Header title={t("Ports and processes")} back={back} />
				<View style={styles.content}>
					<Text style={styles.muted}>{t("This agent is gone.")}</Text>
				</View>
			</View>
		);
	}

	const name = device !== null ? deviceName(view, device) : "";
	const sections = workspace ? portSections(view, workspace, mine) : null;
	const link = device !== null ? linkForDevice(view, device) : undefined;
	const network = link?.network ? view.tables.network.get(link.network) : undefined;
	const where = remote
		? t("On {{device}}, over {{value2}}.", {
				device: name,
				value2: network?.kind.type === "tailscale" ? "Tailscale" : "SSH",
			})
		: t("On this Mac.");
	const agents = workspace
		? all(view, "agent").filter((a) => a.workspace === workspace.id && a.parent === null).length
		: 0;
	const offers = remote && forwards && !offline;
	const offered = sections ? sections.listening.length + sections.others.length : 0;

	return (
		<View style={styles.screen}>
			<Header
				title={t("Ports and processes")}
				back={back}
				subtitle={
					workspace ? (
						<Text numberOfLines={1} style={styles.subtitle}>
							{t("Workspace {{place}} on {{device}}", {
								place: placeName(workspace),
								device: name,
							})}
						</Text>
					) : undefined
				}
			/>
			<ConnectionBanner />
			<ScrollView contentContainerStyle={styles.content} refreshControl={refreshControl}>
				{workspace ? (
					<Text style={styles.intro}>
						{t("{{where}} {{countText}} agents work here.", {
							where,
							count: agents,
							countText: formatNumber(agents),
						})}
						{remote && !forwards
							? t(" {{device}} doesn't forward ports.", { device: name })
							: null}
						{forwards && offline
							? t(
									" {{device}} can't be reached right now. Forwards pick up again once it's back.",
									{ device: name }
								)
							: null}
					</Text>
				) : null}

				{sections && sections.forwarded.length > 0 ? (
					<Section title={t("Forwarded ports")}>
						{sections.forwarded.map(({ port, forward }, i) => (
							<ForwardedRow
								key={port.id}
								port={port}
								forward={forward}
								first={i === 0}
								actions={actions}
								onOpen={setPage}
							/>
						))}
					</Section>
				) : null}
				{sections && sections.listening.length > 0 ? (
					<Section title={remote ? t("Listening, not forwarded") : t("Listening")}>
						{sections.listening.map((port, i) => (
							<ListeningRow
								key={port.id}
								port={port}
								first={i === 0}
								forwards={offers}
								lets={lets}
								actions={actions}
							/>
						))}
					</Section>
				) : null}
				{sections && sections.others.length > 0 ? (
					<Section title={t("Other ports")}>
						{sections.others.map((port, i) => (
							<ListeningRow
								key={port.id}
								port={port}
								first={i === 0}
								quiet
								forwards={offers}
								lets={lets}
								actions={actions}
							/>
						))}
					</Section>
				) : null}
				{offers && !lets && offered > 0 ? <NeedsSwitch /> : null}
				{sections &&
				sections.forwarded.length === 0 &&
				sections.listening.length === 0 &&
				sections.others.length === 0 ? (
					<Text style={styles.muted}>{t("Nothing listens on a port here yet.")}</Text>
				) : null}

				{live ? (
					<Processes
						agent={id}
						read={read}
						failure={failure}
						stops={stops}
						lets={lets}
						ended={turnEnded(view, id)}
					/>
				) : null}
			</ScrollView>
			<PortPage local={page} onClose={() => setPage(null)} />
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { padding: 14, gap: 14, paddingBottom: 40 },
	subtitle: { color: color.mutedForeground, fontSize: size.meta, marginTop: 1 },
	intro: { color: color.secondaryForeground, fontSize: size.small, lineHeight: 19 },
	muted: { color: color.mutedForeground, fontSize: size.small },
	section: { gap: 0 },
	sectionTitle: {
		height: 30,
		lineHeight: 30,
		color: color.mutedForeground,
		fontSize: size.tiny,
		fontWeight: "600",
		paddingHorizontal: 4,
	},
	card: {
		borderRadius: radius.lg,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.card,
		overflow: "hidden",
	},
	row: {
		minHeight: 40,
		justifyContent: "center",
		paddingVertical: 6,
		paddingRight: 12,
		paddingLeft: 14,
		gap: 3,
	},
	rowTop: { flexDirection: "row", alignItems: "center", gap: 8 },
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	grow: { flex: 1, minWidth: 0 },
	portNumber: {
		minWidth: 44,
		color: color.foreground,
		fontFamily: font.mono,
		fontSize: size.small,
		fontVariant: ["tabular-nums"],
	},
	name: { color: color.foreground, fontSize: size.small },
	quiet: { color: color.mutedForeground },
	command: { color: color.mutedForeground, fontFamily: font.mono, fontSize: size.tiny },
	note: { color: color.mutedForeground, fontSize: size.meta },
	failure: { color: color.errorText, fontSize: size.meta },
	error: { color: color.errorText },
	iconButton: {
		width: 32,
		height: 32,
		borderRadius: radius.sm,
		alignItems: "center",
		justifyContent: "center",
	},
	pressed: { backgroundColor: color.accent },
	off: { opacity: 0.4 },
	fold: {
		width: 18,
		height: 18,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: 4,
	},
	processName: { color: color.foreground, fontFamily: font.mono, fontSize: size.meta },
	chip: {
		overflow: "hidden",
		borderRadius: 4,
		backgroundColor: color.secondary,
		color: color.secondaryForeground,
		fontFamily: font.mono,
		fontSize: size.pill,
		paddingHorizontal: 4,
		paddingVertical: 1,
	},
	meta: { color: color.mutedForeground, fontSize: size.pill, fontVariant: ["tabular-nums"] },
	metaUnder: { paddingLeft: 26 },
	mark: { fontStyle: "italic" },
	empty: {
		color: color.mutedForeground,
		fontSize: size.meta,
		paddingHorizontal: 14,
		paddingVertical: 10,
	},
	loading: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 14,
		paddingVertical: 10,
	},
	group: {
		height: 30,
		flexDirection: "row",
		alignItems: "center",
		gap: 4,
		paddingHorizontal: 14,
	},
	groupText: { color: color.mutedForeground, fontSize: size.meta },
	groupCount: {
		color: color.mutedForeground,
		fontSize: size.meta,
		fontVariant: ["tabular-nums"],
	},
	switchNote: { paddingHorizontal: 14, paddingBottom: 10 },
});
