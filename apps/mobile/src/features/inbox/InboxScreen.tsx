// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The first screen, laid out as the desktop's projects view: the cards
// devices raised about programs asking to get in, which you answer on the
// Mac, then a box of the agents waiting on you with their answers, then
// the pinned ones, then one
// group per project, then the snoozed and settled ones. A project's group
// ends with its Notes row, which opens its notes, with a plus for a new
// one, while it has notes or a connected device can take one. A row slides left
// to settle or snooze it where its device keeps inboxes, and a long press
// opens the rest of its menu. Pulling the list down tries the devices
// again. Before the phone is paired with anything, it shows how to pair
// instead. With several Macs each row carries its Mac's tag, or a switch on
// top shows one Mac at a time.

import { t, useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";
import {
	FlatList,
	KeyboardAvoidingView,
	Platform,
	Pressable,
	StyleSheet,
	Text,
	View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { Admission, Agent, ProjectId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { agentLook } from "@maxhq/shared/status/activity";
import { formatAgoShort } from "@maxhq/shared/status/format";
import { deviceReach } from "@maxhq/shared/status/reach";
import {
	changeInbox,
	dismissFailures,
	undoNotice,
	watchAutoSettles,
} from "@maxhq/shared/ui/inbox-notice";
import { openAdmissions } from "@maxhq/shared/ui/admission";
import type { InboxStep } from "@maxhq/shared/view/inbox-actions";
import { inboxList, type InboxRow, swipeMenu } from "@maxhq/shared/view/inbox-list";
import { notesEntry, remoteNotesReach } from "@maxhq/shared/view/notes";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName, type NeedsYouItem } from "@maxhq/shared/view/select";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, radius, size, tint, TOUCH } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { ActivityDot, ConnectionDot } from "../../ui/Dot";
import { Header, IconButton } from "../../ui/Header";
import { Chevron, Gear, Plus } from "../../ui/Icon";
import { Pill } from "../../ui/Pill";
import { usePullToRefresh } from "../../ui/Refresh";
import { type SwipeAction, SwipeRow } from "../../ui/SwipeRow";
import { AdmissionCard } from "../attention/AdmissionCard";
import { AttentionCard } from "../attention/AttentionCard";
import { promptOfEvent } from "../attention/prompt";
import { createRenameDialog } from "../agent/more";
import { tagOf } from "../macs/macs";
import { MacSwitch } from "../macs/MacSwitch";
import { MacTag } from "../macs/MacTag";
import { useMacs, useMacView } from "../macs/useMacs";
import { RenameDialog } from "../agent/RenameDialog";
import { renamesOf } from "../agent/renames";
import { reachSummary } from "../devices/summary";
import { Dispatcher } from "../dispatcher/Dispatcher";
import { openNewNote } from "../notes/NotesScreen";
import { UsageButton } from "../usage/UsageButton";
import { FirstRun } from "./FirstRun";
import { openInboxSheet } from "./InboxSheet";
import { noticesOf, useNotices } from "./notices";
import { loudRow, phoneRowMeta, rowSubtitle } from "./rows";
import { pickSnooze } from "./SnoozeSheet";

/** One entry of the list, top to bottom. */
type Item =
	| { readonly kind: "admission"; readonly key: string; readonly admission: Admission }
	| { readonly kind: "needs"; readonly key: string; readonly items: readonly NeedsYouItem[] }
	| {
			readonly kind: "group";
			readonly key: string;
			readonly name: string;
			readonly count: number;
			readonly open: boolean;
	  }
	| { readonly kind: "row"; readonly key: string; readonly row: InboxRow }
	| {
			readonly kind: "notes";
			readonly key: string;
			readonly project: ProjectId;
			/** How many live notes it has. */
			readonly count: number;
			/** Whether a new note can be made. */
			readonly writable: boolean;
	  }
	| {
			readonly kind: "more";
			readonly key: string;
			/** The group it opens, or "settled". */
			readonly of: string;
			readonly label: string;
			readonly open: boolean;
	  };

const SETTLED = "settled";
// How many device dots the header's button shows before it lets the count speak.
const HEADER_DOTS = 6;

/** The list's entries: the groups you closed show only their heading. */
const itemsOf = (
	view: ViewState,
	now: number,
	closed: ReadonlySet<string>,
	shown: ReadonlySet<string>
): Item[] => {
	const list = inboxList(view, now);
	const items: Item[] = [];
	for (const admission of openAdmissions(view)) {
		items.push({ kind: "admission", key: `a:${admission.id}`, admission });
	}
	if (list.waiting.length > 0) items.push({ kind: "needs", key: "needs", items: list.waiting });
	const group = (
		key: string,
		name: string,
		rows: readonly InboxRow[],
		older: readonly InboxRow[],
		project: ProjectId | null = null
	) => {
		const notes = project ? notesEntry(view, project, remoteNotesReach(view, project)) : null;
		if (rows.length + older.length === 0 && !notes) return;
		const open = !closed.has(key);
		items.push({
			kind: "group",
			key: `g:${key}`,
			name,
			count: rows.length + older.length,
			open,
		});
		if (!open) return;
		for (const row of rows) items.push({ kind: "row", key: row.agent.id, row });
		if (older.length > 0) {
			const more = shown.has(key);
			items.push({
				kind: "more",
				key: `o:${key}`,
				of: key,
				label: more ? t("Hide older") : t("{{count}} older", { count: older.length }),
				open: more,
			});
			if (more) for (const row of older) items.push({ kind: "row", key: row.agent.id, row });
		}
		if (project && notes) items.push({ kind: "notes", key: `n:${key}`, project, ...notes });
	};
	group("pinned", t("Pinned"), list.pinned, []);
	for (const g of list.groups) group(g.key, g.name, g.rows, g.older, g.project);
	group(
		"snoozed",
		t("Snoozed"),
		list.snoozed.map((agent) => ({ agent, level: 0 })),
		[]
	);
	if (list.settled.length > 0) {
		const more = shown.has(SETTLED);
		items.push({
			kind: "more",
			key: "settled",
			of: SETTLED,
			label: t("{{count}} settled", { count: list.settled.length }),
			open: more,
		});
		if (more) {
			for (const agent of list.settled) {
				items.push({ kind: "row", key: agent.id, row: { agent, level: 0 } });
			}
		}
	}
	return items;
};

const toggle = (set: ReadonlySet<string>, key: string): Set<string> => {
	const next = new Set(set);
	if (!next.delete(key)) next.add(key);
	return next;
};

export const InboxScreen = ({ navigation }: ScreenProps<"Inbox">) => {
	const { t } = useI18n();

	const services = useServices();
	const whole = useView((s) => s.view);
	// Showing one Mac at a time, the list holds only that Mac's.
	const macs = useMacs();
	const view = useMacView(whole, macs);
	const status = useView((s) => s.status);
	const now = useNow(60_000);
	const [closed, setClosed] = useState<ReadonlySet<string>>(new Set());
	const [shown, setShown] = useState<ReadonlySet<string>>(new Set());
	const notices = noticesOf(services);
	const insets = useSafeAreaInsets();
	const [dialog] = useState(() => createRenameDialog(renamesOf(services)));
	const refreshControl = usePullToRefresh(() => services.source.refreshLinks(null));
	useEffect(() => watchAutoSettles(notices), [notices]);

	const devices = deviceReach(view, now);
	const items = itemsOf(view, now, closed, shown);
	const unpaired =
		status.state !== "loading" &&
		(view === whole ? devices : deviceReach(whole, now)).length === 0;

	const open = (agent: Agent) =>
		navigation.navigate("Agent", {
			agent: agent.id,
			attention:
				agent.activity.state === "needs_input" ? agent.activity.attention : undefined,
		});

	const change = (steps: readonly InboxStep[]) => void changeInbox(notices, steps);

	const actionsOf = (agent: Agent): SwipeAction[] => {
		const menu = swipeMenu(view, agent, now);
		if (!menu) return [];
		const actions: SwipeAction[] = [];
		if (menu.wake) {
			const wake = menu.wake;
			actions.push({
				label: t("Wake"),
				tone: "secondary",
				onPress: () => change(wake.steps),
			});
		} else if (menu.snooze) {
			const ids = menu.snooze;
			actions.push({
				label: t("Snooze"),
				tone: "secondary",
				onPress: () => pickSnooze(notices, ids, menu.untilLimit),
			});
		}
		if (menu.settle) {
			const settle = menu.settle;
			actions.push({
				label: settle.label,
				tone: "primary",
				onPress: () => change(settle.steps),
			});
		}
		return actions;
	};

	const placeOf = (agent: Agent) => {
		const workspace = view.tables.workspace.get(agent.workspace);
		return workspace ? deviceName(view, workspace.device) : t("Unknown device");
	};
	// The Mac tag on a row, while several Macs show together.
	const tagFor = (agent: Agent) => tagOf(whole, macs.places, macs.mode, agent);

	const renderItem = ({ item }: { item: Item }) => {
		switch (item.kind) {
			case "admission":
				return <AdmissionCard admission={item.admission} view={view} />;
			case "needs":
				return (
					<NeedsYou
						items={item.items}
						placeOf={placeOf}
						tagOf={tagFor}
						onOpen={open}
						now={now}
					/>
				);
			case "group":
				return (
					<Pressable
						accessibilityRole="button"
						accessibilityState={{ expanded: item.open }}
						accessibilityLabel={
							item.count > 0
								? t("{{name}}, {{count}} agents", {
										name: item.name,
										count: item.count,
									})
								: item.name
						}
						onPress={() => setClosed((set) => toggle(set, item.key.slice(2)))}
						style={styles.group}
					>
						<Chevron dir={item.open ? "down" : "right"} size={12} />
						<Text style={styles.groupName} numberOfLines={1}>
							{item.name}
						</Text>
						{item.count > 0 ? (
							<Text style={styles.groupCount}>{formatNumber(item.count)}</Text>
						) : null}
					</Pressable>
				);
			case "notes": {
				const { project } = item;
				return (
					<View style={styles.notes}>
						<Pressable
							accessibilityRole="link"
							accessibilityLabel={
								item.count > 0
									? t("Notes, {{count}} notes", { count: item.count })
									: t("Notes")
							}
							onPress={() => navigation.navigate("Notes", { project })}
							style={({ pressed }) => [styles.notesOpen, pressed && styles.pressed]}
						>
							<Text style={styles.notesLabel}>{t("Notes")}</Text>
							{item.count > 0 ? (
								<Text style={styles.groupCount}>{formatNumber(item.count)}</Text>
							) : null}
						</Pressable>
						{item.writable ? (
							<IconButton
								label={t("New note")}
								onPress={() =>
									void openNewNote(services, project, (note) =>
										navigation.navigate("Note", { project, note, made: true })
									)
								}
							>
								<Plus size={13} tint={color.mutedForeground} />
							</IconButton>
						) : null}
					</View>
				);
			}
			case "more":
				return (
					<Pressable
						accessibilityRole="button"
						accessibilityState={{ expanded: item.open }}
						onPress={() => setShown((set) => toggle(set, item.of))}
						style={[styles.more, item.of !== SETTLED && styles.moreInGroup]}
					>
						<Chevron dir={item.open ? "down" : "right"} size={12} />
						<Text style={styles.moreLabel}>{item.label}</Text>
					</Pressable>
				);
			case "row": {
				const { agent, level } = item.row;
				return (
					<AgentRow
						agent={agent}
						level={level}
						place={placeOf(agent)}
						tag={tagFor(agent)}
						actions={actionsOf(agent)}
						onOpen={open}
						onMore={() => openInboxSheet(services, agent, dialog)}
						view={view}
						now={now}
					/>
				);
			}
		}
	};

	const body = unpaired ? (
		<FirstRun
			onScan={() => navigation.navigate("Scan")}
			onByHand={() => navigation.navigate("Settings", { byHand: true })}
		/>
	) : (
		<FlatList
			data={items}
			keyExtractor={(item) => item.key}
			renderItem={renderItem}
			contentContainerStyle={styles.content}
			keyboardShouldPersistTaps="handled"
			refreshControl={refreshControl}
			ListHeaderComponent={<MacSwitch view={whole} macs={macs} />}
			ListEmptyComponent={
				<Text style={styles.note}>
					{status.state === "loading"
						? t("Loading")
						: t("No agents yet. Start one on the Mac, and it shows here.")}
				</Text>
			}
		/>
	);
	// The dispatcher's bar keeps the home indicator clear when it's there.
	const notice = (
		<NoticeBar
			bottom={(devices.length === 0 ? insets.bottom : 0) + 12}
			onUndo={() => undoNotice(notices)}
			onDismiss={() => dismissFailures(notices)}
		/>
	);

	// The dispatcher's bar sits under the list once there's a device. Its
	// safe area pad hides behind the keyboard, so the box sits on top of it.
	return (
		<KeyboardAvoidingView
			style={styles.screen}
			behavior={Platform.OS === "ios" ? "padding" : undefined}
			keyboardVerticalOffset={-insets.bottom}
		>
			<Header
				title={t("Agents")}
				right={
					<>
						{devices.length > 0 ? (
							<UsageButton onPress={() => navigation.navigate("Usage")} />
						) : null}
						{devices.length > 0 ? (
							<Pressable
								accessibilityRole="button"
								accessibilityLabel={t("Devices: {{summary}}", {
									summary: reachSummary(devices),
								})}
								hitSlop={{ top: 6, bottom: 6 }}
								onPress={() => navigation.navigate("Devices")}
								style={({ pressed }) => [styles.devices, pressed && styles.pressed]}
							>
								<View style={styles.dots}>
									{devices.slice(0, HEADER_DOTS).map((device) => (
										<ConnectionDot
											key={device.id}
											look={device.look}
											size={7}
											surface={color.secondary}
											decorative
										/>
									))}
								</View>
								<Text style={styles.devicesLabel}>
									{t("{{count}} devices", { count: devices.length })}
								</Text>
							</Pressable>
						) : null}
						<IconButton
							label={t("Settings")}
							onPress={() => navigation.navigate("Settings")}
						>
							<Gear />
						</IconButton>
					</>
				}
			/>
			<ConnectionBanner />
			{status.state === "failed" ? <Text style={styles.note}>{status.message}</Text> : null}
			{devices.length === 0 ? (
				<>
					{body}
					{notice}
				</>
			) : (
				<Dispatcher onOpenAgent={(agent) => navigation.navigate("Agent", { agent })}>
					{body}
					{notice}
				</Dispatcher>
			)}
			<RenameDialog dialog={dialog} />
		</KeyboardAvoidingView>
	);
};

/** The green box of agents waiting on you, each with its prompt when the view has it. */
const NeedsYou = ({
	items,
	placeOf,
	tagOf,
	onOpen,
	now,
}: {
	items: readonly NeedsYouItem[];
	placeOf: (agent: Agent) => string;
	tagOf: (agent: Agent) => string | null;
	onOpen: (agent: Agent) => void;
	now: number;
}) => {
	const { t } = useI18n();
	return (
		<View style={styles.needs} accessibilityLabel={t("Needs you")}>
			<View style={styles.needsHead}>
				<Text accessibilityRole="header" style={styles.needsTitle}>
					{t("Needs you")}
				</Text>
				<View style={styles.needsCount}>
					<Text style={styles.needsCountText}>{formatNumber(items.length)}</Text>
				</View>
			</View>
			{items.map(({ agent, attention }) => {
				const tag = tagOf(agent);
				return attention ? (
					<AttentionCard
						key={agent.id}
						prompt={promptOfEvent(attention)}
						vendor={agent.vendor}
						title={agent.title}
						place={placeOf(agent)}
						tag={tag ? <MacTag name={tag} /> : undefined}
						onOpen={() => onOpen(agent)}
					/>
				) : (
					<Pressable
						key={agent.id}
						accessibilityRole="link"
						onPress={() => onOpen(agent)}
						style={({ pressed }) => [styles.ask, pressed && styles.pressed]}
					>
						<ActivityDot
							look="needs_input"
							surface={tint(color.ready, 0.07)}
							decorative
						/>
						<View style={styles.text}>
							<Text style={styles.title} numberOfLines={1}>
								{agent.title}
							</Text>
							<Text style={styles.askSub} numberOfLines={1}>
								{t("Waiting on you, on {{device}}", { device: placeOf(agent) })}
							</Text>
						</View>
						{tag ? <MacTag name={tag} /> : null}
						<Text style={styles.meta}>
							{formatAgoShort(agent.last_activity_at, now)}
						</Text>
					</Pressable>
				);
			})}
		</View>
	);
};

const AgentRow = ({
	agent,
	level,
	place,
	tag,
	actions,
	onOpen,
	onMore,
	view,
	now,
}: {
	agent: Agent;
	level: number;
	place: string;
	/** The short name of the agent's Mac, while several Macs show together. */
	tag: string | null;
	actions: readonly SwipeAction[];
	onOpen: (agent: Agent) => void;
	onMore: () => void;
	view: ViewState;
	now: number;
}) => {
	const { t } = useI18n();

	const look = agentLook(agent, now);
	const meta = phoneRowMeta(view, agent, now);
	return (
		<SwipeRow
			actions={actions}
			onPress={() => onOpen(agent)}
			onLongPress={onMore}
			label={agent.title}
		>
			<View style={[styles.row, { paddingLeft: 10 + level * 16 }]}>
				<ActivityDot look={look} />
				<View style={styles.text}>
					<Text style={[styles.title, !loudRow(look) && styles.quiet]} numberOfLines={1}>
						{agent.title}
					</Text>
					<Text style={styles.sub} numberOfLines={1}>
						{rowSubtitle(agent.vendor, place, look)}
					</Text>
				</View>
				{tag ? <MacTag name={tag} /> : null}
				{meta.kind === "input" ? (
					<Pill label={t("Input")} tone="ready" />
				) : (
					<Text style={[styles.meta, meta.kind === "asleep" && styles.asleep]}>
						{meta.text}
					</Text>
				)}
			</View>
		</SwipeRow>
	);
};

/** The last change with its Undo, or what went wrong. */
const NoticeBar = ({
	bottom,
	onUndo,
	onDismiss,
}: {
	bottom: number;
	onUndo: () => void;
	onDismiss: () => void;
}) => {
	const { t } = useI18n();

	const notice = useNotices((s) => s.notice);
	const failures = useNotices((s) => s.failures);
	if (failures.length > 0) {
		return (
			<Pressable
				accessibilityRole="alert"
				onPress={onDismiss}
				style={[styles.notice, { bottom }]}
			>
				<Text style={styles.failed}>{failures.join(" ")}</Text>
			</Pressable>
		);
	}
	if (!notice) return null;
	return (
		<View style={[styles.notice, { bottom }]} accessibilityLiveRegion="polite">
			<Text style={styles.noticeText}>{notice.text}</Text>
			<Pressable accessibilityRole="button" onPress={onUndo} style={styles.undo}>
				<Text style={styles.undoLabel}>{t("Undo")}</Text>
			</Pressable>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { paddingHorizontal: 10, paddingTop: 12, paddingBottom: 96, gap: 0 },
	pressed: { backgroundColor: color.accent },
	devices: {
		height: 32,
		flexDirection: "row",
		alignItems: "center",
		gap: 7,
		paddingHorizontal: 10,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	dots: { flexDirection: "row", alignItems: "center", gap: 3 },
	devicesLabel: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "500" },
	needs: {
		marginBottom: 12,
		padding: 6,
		gap: 4,
		borderRadius: 10,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: tint(color.ready, 0.22),
		backgroundColor: tint(color.ready, 0.07),
	},
	needsHead: {
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 6,
		paddingVertical: 3,
	},
	needsTitle: { color: color.readyText, fontSize: size.meta, fontWeight: "600" },
	needsCount: {
		minWidth: 16,
		height: 16,
		paddingHorizontal: 4,
		borderRadius: 8,
		backgroundColor: tint(color.ready, 0.18),
		alignItems: "center",
		justifyContent: "center",
	},
	needsCountText: { color: color.readyText, fontSize: size.pill, fontWeight: "600" },
	ask: {
		minHeight: 48,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingLeft: 10,
		paddingRight: 8,
		borderRadius: radius.sm,
	},
	askSub: { color: color.readyText, fontSize: size.meta },
	group: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 8,
		marginBottom: -8,
	},
	groupName: { flex: 1, color: color.mutedForeground, fontSize: size.meta, fontWeight: "600" },
	groupCount: { color: color.mutedForeground, fontSize: size.meta, fontWeight: "500" },
	more: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 8,
	},
	moreInGroup: { paddingLeft: 30 },
	notes: { flexDirection: "row", alignItems: "center" },
	notesOpen: {
		flex: 1,
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingLeft: 30,
		paddingRight: 8,
		borderRadius: radius.sm,
	},
	notesLabel: { flex: 1, color: color.secondaryForeground, fontSize: 12.5 },
	moreLabel: { color: color.mutedForeground, fontSize: 12.5 },
	row: { minHeight: 46, flexDirection: "row", alignItems: "center", gap: 10, paddingRight: 8 },
	text: { flex: 1, minWidth: 0, gap: 1 },
	title: { color: color.foregroundStrong, fontSize: size.body },
	quiet: { color: color.terminal },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	meta: { color: color.mutedForeground, fontSize: size.meta },
	asleep: { color: color.subtleForeground },
	note: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	notice: {
		position: "absolute",
		left: 12,
		right: 12,
		flexDirection: "row",
		alignItems: "center",
		gap: 12,
		paddingLeft: 14,
		paddingRight: 4,
		minHeight: 48,
		borderRadius: radius.md,
		backgroundColor: color.secondary,
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
	},
	noticeText: { flex: 1, color: color.foreground, fontSize: size.small, paddingVertical: 10 },
	undo: { minHeight: TOUCH, minWidth: TOUCH, paddingHorizontal: 10, justifyContent: "center" },
	undoLabel: { color: color.primary, fontSize: size.small, fontWeight: "600" },
	failed: { flex: 1, color: color.errorText, fontSize: size.small, paddingVertical: 12 },
});
