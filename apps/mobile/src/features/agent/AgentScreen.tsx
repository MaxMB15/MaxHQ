// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One agent: its feed, newest at the bottom, drawn as the desktop draws it,
// with replies and your messages as Markdown, each tool step as one line that
// opens to show more, and its approvals and questions as cards, then a box to
// send it a message. Opened for a prompt, it scrolls to that prompt's card, or to the
// end when the feed has none. Looking at a done agent marks it seen. Until
// the devices answer, an agent the view doesn't have yet shows Loading.
// Pulling the feed down tries the devices again and reloads its newest
// page. Scrolled up, a Latest button takes it back to the end. Find, from
// the header or More, searches the whole feed and goes to each match, and
// Your prompts, from More, jumps to a prompt you sent. The header's second
// line opens the agent's details. Over the composer sit the clone, moved,
// resume and port banners, as on the desktop.

import { useI18n } from "@maxhq/shared/i18n";

import { useIsFocused, useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import { useCallback, useEffect, useRef, useState } from "react";
import {
	FlatList,
	Keyboard,
	KeyboardAvoidingView,
	Platform,
	Pressable,
	StyleSheet,
	Text,
	View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";

import { canAllow } from "@maxhq/shared/agent-tools/scope";
import { FindProvider } from "@maxhq/shared/agent-view/find-context";
import {
	finishedLead,
	finishedNote,
	subAgentLead,
	useKeepSubAgent,
} from "@maxhq/shared/agent-view/finished";
import { movedTo } from "@maxhq/shared/agent-view/outside";
import type { Agent, AgentId, AttentionId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { ACTIVITY_LABEL, agentLook } from "@maxhq/shared/status/activity";
import { formatClock } from "@maxhq/shared/status/format";
import { VENDOR_NAME } from "@maxhq/shared/status/usage";
import {
	type FeedAway,
	feedAway,
	type FeedRow,
	feedIndexOf,
	rowsSince,
} from "@maxhq/shared/timeline/output";
import { useTimeline } from "@maxhq/shared/timeline/use-timeline";
import { lookAt } from "@maxhq/shared/view/seen";
import { attentionOf, deviceName } from "@maxhq/shared/view/select";

import type { ScreenProps, Screens } from "../../navigation";
import { useAppActive, useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { ConnectionBanner } from "../../ui/ConnectionBanner";
import { Header } from "../../ui/Header";
import { usePullToRefresh } from "../../ui/Refresh";
import { AttentionCard } from "../attention/AttentionCard";
import { promptOfEvent } from "../attention/prompt";
import { PullChips } from "../pulls";
import { Automations } from "../tasks";
import { LeaseChips } from "../test-hosts/LeaseChips";
import { AgentMore } from "./AgentMore";
import { CloneBanner } from "./CloneBanner";
import { hasComposer } from "./compose";
import { Composer } from "./Composer";
import { ExportBar, useConversationExport } from "./ExportConversation";
import { FeedProvider, useFeedRows } from "./feed-context";
import { FindBar, FindButton, useFeedFind } from "./FeedFind";
import { FeedItem } from "./FeedRow";
import { FinishedAgent } from "./FinishedAgent";
import { PlaceLine, SkillsChip } from "./HeaderLine";
import { Latest } from "./Latest";
import { MovedBanner } from "./MovedBanner";
import { Divider } from "./Notices";
import { feedScroll, missingAgent } from "./opening";
import { OutsideBanner, SessionMoved } from "./OutsideBanner";
import { loadPermissions, permissions, useHeldPermission } from "./permissions";
import { PortOffers } from "./PortOffers";
import { PromptsSheet } from "./PromptsSheet";
import { useShownTitle } from "./renames";
import { ResumeBanner } from "./ResumeBanner";

// How close to the bottom counts as reading the newest rows, so new ones
// keep the feed at the end.
const AT_END = 80;

type Back = { label: string; onPress: () => void };

export const AgentScreen = ({ navigation, route }: ScreenProps<"Agent">) => {
	const { t } = useI18n();

	const agent = useView((s) => s.view.tables.agent.get(route.params.agent));
	const back = { label: t("Back to agents"), onPress: () => navigation.goBack() };

	if (!agent) return <MissingAgent id={route.params.agent} back={back} />;
	return <AgentBody agent={agent} attention={route.params.attention} back={back} />;
};

/**
 * An agent the view doesn't have: Loading while the devices may still send
 * it, then gone. A sub-agent gone with its session points to where the
 * session went, and one that finished shows its history.
 */
const MissingAgent = ({ id, back }: { id: AgentId; back: Back }) => {
	const { t } = useI18n();

	const status = useView((s) => s.status);
	const view = useView((s) => s.view);
	const moved = movedTo(view.tables.agent, id);
	const lead = moved ? null : finishedLead(view, id);
	const loading = missingAgent(status, view, useNow(1_000)) === "loading";
	if (lead) return <FinishedAgent id={id} lead={lead} back={back} />;
	return (
		<View style={styles.screen}>
			<Header title="" back={back} />
			<ConnectionBanner />
			{moved ? (
				<View style={styles.gone}>
					<SessionMoved lead={moved} />
				</View>
			) : (
				<Text style={[styles.note, styles.gone]}>
					{loading ? t("Loading") : (finishedNote(view, id) ?? t("This agent is gone."))}
				</Text>
			)}
		</View>
	);
};

/** Whether the keyboard is up, from the moment it starts to show on iOS. */
const useKeyboardShown = (): boolean => {
	const [shown, setShown] = useState(false);
	useEffect(() => {
		const ios = Platform.OS === "ios";
		const show = Keyboard.addListener(ios ? "keyboardWillShow" : "keyboardDidShow", () =>
			setShown(true)
		);
		const hide = Keyboard.addListener(ios ? "keyboardWillHide" : "keyboardDidHide", () =>
			setShown(false)
		);
		return () => {
			show.remove();
			hide.remove();
		};
	}, []);
	return shown;
};

/** Marks the agent seen while its screen is in front and the app is open. */
const useLookingAt = (agent: Agent) => {
	const services = useServices();
	const focused = useIsFocused();
	const active = useAppActive();
	const looking = focused && active;
	// Coming back to the agent tries a failed clear again; staying on it doesn't.
	const fresh = useRef(true);
	useEffect(() => {
		if (!looking) {
			fresh.current = true;
			return;
		}
		lookAt(services, agent.id, fresh.current);
		fresh.current = false;
	}, [looking, services, agent]);
};

const AgentBody = ({
	agent,
	attention,
	back,
}: {
	agent: Agent;
	attention?: AttentionId;
	back: Back;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	useLookingAt(agent);
	const view = useView((s) => s.view);
	// So the screen can show the history once the sub-agent finishes.
	useKeepSubAgent(agent, subAgentLead(view.tables.agent, agent));
	const timeline = useTimeline(source, agent.id);
	const status = useStore(timeline.store, (s) => s.status);
	const error = useStore(timeline.store, (s) => s.error);
	const hasOlder = useStore(timeline.store, (s) => s.hasOlder);
	const loadingOlder = useStore(timeline.store, (s) => s.loadingOlder);
	const queued = useStore(timeline.store, (s) => s.queued);
	const rows = useFeedRows(timeline.store);
	const asking = attentionOf(view, agent);
	const workspace = view.tables.workspace.get(agent.workspace);
	const device = workspace ? deviceName(view, workspace.device) : t("this device");
	const project = workspace?.project
		? view.tables.project.get(workspace.project)?.name
		: undefined;
	const look = agentLook(agent, useNow(10_000));
	const place = [
		ACTIVITY_LABEL[look],
		t("{{vendor}} on {{device}}", { vendor: VENDOR_NAME[agent.vendor], device }),
		...(project ? [project] : []),
	].join(", ");
	const title = useShownTitle(agent);
	const insets = useSafeAreaInsets();
	const keyboard = useKeyboardShown();
	const list = useRef<FlatList<FeedRow>>(null);
	// Opened for a prompt, the feed waits at that card instead of the end.
	const atEnd = useRef(attention === undefined);
	// The prompt the feed last went to, so each push moves it once.
	const went = useRef<AttentionId | undefined>(undefined);
	const found = useFeedFind(agent.id, timeline, rows, list, atEnd);
	const [away, setAway] = useState<FeedAway>(null);
	// Your prompts, open with the row at the top of the feed as it opened.
	const [prompts, setPrompts] = useState<{ row: string | undefined } | null>(null);
	const exporting = useConversationExport(agent);
	const held = useHeldPermission(agent);
	const allows = canAllow(agent);
	useEffect(() => {
		if (allows) void loadPermissions(permissions, source);
	}, [allows, source]);
	const refreshControl = usePullToRefresh(() =>
		Promise.all([source.refreshLinks(null), timeline.reload()])
	);

	const scroll = feedScroll(rows, attention, status !== "loading");
	useEffect(() => {
		if (scroll === null || went.current === attention) return;
		went.current = attention;
		atEnd.current = scroll === "end";
		if (scroll === "end") list.current?.scrollToEnd({ animated: false });
		else list.current?.scrollToIndex({ index: scroll, viewPosition: 0.3, animated: false });
	}, [scroll, attention]);

	// The prompt the agent waits on, when the feed has no card for it yet.
	const pending = asking && feedIndexOf(rows, asking.id) < 0 ? asking : undefined;
	const { goal } = found;
	const renderItem = useCallback(
		({ item }: { item: FeedRow }) => <FeedItem id={item.id} gap={item.gap} goal={goal} />,
		[goal]
	);
	const observed = agent.control === "observed";
	const headed =
		hasOlder ||
		status === "loading" ||
		Boolean(error) ||
		observed ||
		Boolean(agent.moved?.from);

	return (
		<KeyboardAvoidingView
			style={styles.screen}
			behavior={Platform.OS === "ios" ? "padding" : undefined}
			keyboardVerticalOffset={0}
		>
			<Header
				title={title}
				back={back}
				right={
					<>
						<FindButton find={found.find} />
						<AgentMore
							agent={agent}
							held={held}
							onFind={found.find.open}
							onPrompts={() => setPrompts({ row: found.topRow() })}
							onExport={exporting.run}
							onDeleted={back.onPress}
						/>
					</>
				}
				subtitle={
					<PlaceLine
						agent={agent}
						look={look}
						place={place}
						onPress={() => navigation.navigate("AgentInfo", { agent: agent.id })}
					/>
				}
			/>
			<FindBar find={found.find} />
			<PullChips agent={agent} />
			<Automations agent={agent} />
			<LeaseChips agent={agent.id} bar />
			<SkillsChip agent={agent.id} />
			{exporting.loading ? <ExportBar /> : null}
			<ConnectionBanner />
			<FindProvider store={found.find.store}>
				<FeedProvider agent={agent} device={device} store={timeline.store}>
					<FlatList
						ref={list}
						data={rows}
						keyExtractor={(row) => row.id}
						contentContainerStyle={styles.content}
						renderItem={renderItem}
						// Rows are short, so a screen holds many of them.
						initialNumToRender={24}
						onViewableItemsChanged={found.onViewableItemsChanged}
						ListHeaderComponent={
							<View style={[styles.head, headed ? styles.headed : null]}>
								{hasOlder ? (
									<Pressable
										accessibilityRole="button"
										disabled={loadingOlder}
										onPress={timeline.loadOlder}
										style={styles.olderButton}
									>
										<Text style={styles.older}>
											{loadingOlder
												? t("Loading")
												: t("Load earlier activity")}
										</Text>
									</Pressable>
								) : null}
								{status === "loading" ? (
									<Text style={styles.note}>{t("Loading")}</Text>
								) : null}
								{error ? <Text style={styles.failed}>{error}</Text> : null}
								<MovedBanner agent={agent} way="from" />
								{observed && !hasOlder ? (
									<Divider icon={null}>
										{t(
											"Started in a terminal at {{time}}. History comes from the session file.",
											{ time: formatClock(agent.started_at) }
										)}
									</Divider>
								) : null}
							</View>
						}
						ListFooterComponent={
							<>
								{pending ? (
									<View style={styles.pending}>
										<AttentionCard
											prompt={promptOfEvent(pending)}
											vendor={agent.vendor}
											place={device}
										/>
									</View>
								) : null}
								{observed && agent.link.state === "down" ? (
									<View style={styles.pending}>
										<Divider icon={null}>
											{agent.link.since !== null
												? t("Session in the terminal ended at {{time}}", {
														time: formatClock(agent.link.since),
													})
												: t("Session in the terminal ended")}
										</Divider>
									</View>
								) : null}
							</>
						}
						onScroll={({ nativeEvent: e }) => {
							const end =
								e.contentOffset.y + e.layoutMeasurement.height >=
								e.contentSize.height - AT_END;
							atEnd.current = end;
							setAway((was) => feedAway(was, rows, end));
						}}
						scrollEventThrottle={100}
						onContentSizeChange={() => {
							if (atEnd.current) list.current?.scrollToEnd({ animated: false });
						}}
						onScrollToIndexFailed={({ index: at, averageItemLength }) => {
							list.current?.scrollToOffset({
								offset: at * averageItemLength,
								animated: false,
							});
							setTimeout(
								() => list.current?.scrollToIndex({ index: at, viewPosition: 0.3 }),
								100
							);
						}}
						keyboardShouldPersistTaps="handled"
						keyboardDismissMode="interactive"
						refreshControl={refreshControl}
					/>
				</FeedProvider>
			</FindProvider>
			{/* The keyboard covers the home indicator, so its inset applies only while it's down. */}
			<View style={{ paddingBottom: keyboard ? 0 : insets.bottom }}>
				{away ? (
					<Latest
						fresh={rowsSince(away, rows)}
						onPress={() => list.current?.scrollToEnd()}
					/>
				) : null}
				<CloneBanner workspace={agent.workspace} />
				<MovedBanner agent={agent} way="to" />
				{observed ? null : <ResumeBanner agent={agent} />}
				<PortOffers agent={agent} />
				{hasComposer(agent) ? (
					<Composer key={agent.id} agent={agent} device={device} queued={queued} />
				) : (
					<OutsideBanner agent={agent} device={device} />
				)}
			</View>
			<PromptsSheet
				timeline={timeline}
				rows={rows}
				top={prompts}
				onClose={() => setPrompts(null)}
				onJump={(row) => {
					atEnd.current = row === "end";
					if (row === "end") list.current?.scrollToEnd();
					else list.current?.scrollToIndex({ index: row, viewPosition: 0 });
				}}
			/>
		</KeyboardAvoidingView>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { paddingHorizontal: 14, paddingVertical: 14 },
	pending: { marginTop: 12 },
	gone: { padding: 16 },
	head: { gap: 10 },
	headed: { marginBottom: 12 },
	olderButton: { minHeight: 44, justifyContent: "center" },
	older: { color: color.primary, fontSize: size.small },
	note: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText, fontSize: size.meta },
});
