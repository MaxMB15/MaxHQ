// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The dispatcher on the home screen: a bar at the foot with a box to ask
// in, and a sheet that slides up over the list with the conversation, the
// plan it hands back and how the plan went. A swipe up on the bar or a tap
// on its grabber opens the sheet, a swipe down on either closes it. Nothing
// in a plan runs until you confirm it. The dispatcher runs on the Mac, so
// the bar says why it can't take a request while that Mac is out of reach.

import { t, useI18n } from "@maxhq/shared/i18n";

import { type ComponentRef, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import {
	Animated,
	Keyboard,
	LayoutAnimation,
	PanResponder,
	Pressable,
	ScrollView,
	StyleSheet,
	Text,
	TextInput,
	type TextInputInstance,
	View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";

import type { Agent, AgentId, Plan } from "@maxhq/shared/bindings";
import {
	CONFIRM_HINT,
	inConversation,
	isWorking,
	noConversationHint,
	WORKING_HINT,
} from "@maxhq/shared/dispatch/conversation";
import { FIELD_LINES, fieldMaxHeight } from "@maxhq/shared/dispatch/field";
import {
	ask,
	confirm,
	createDispatchFlow,
	dismiss,
	type DispatchFlow,
	type PlanRun,
	setDraft,
	startOver,
} from "@maxhq/shared/dispatch/flow";
import {
	dispatcherTitle,
	itemLines,
	itemPhrase,
	planSummary,
	resultRows,
	resultsTitle,
	stepNeedsSwitch,
} from "@maxhq/shared/dispatch/plan";
import { ANY_PLACE, takesAnswerPills, type Typed } from "@maxhq/shared/pills/answer";
import { sentence } from "@maxhq/shared/status/format";
import { feedIndexOf } from "@maxhq/shared/timeline/output";
import { useTimeline } from "@maxhq/shared/timeline/use-timeline";
import { attentionOf, deviceName } from "@maxhq/shared/view/select";

import { useBackPress } from "../../platform/back";
import { type Services, useServices, useView } from "../../services";
import { color, radius, size, tint, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
import { Chevron } from "../../ui/Icon";
import { useReduceMotion } from "../../ui/motion";
import { FeedProvider, useFeedRows } from "../agent/feed-context";
import { FeedItem } from "../agent/FeedRow";
import { AttentionCard } from "../attention/AttentionCard";
import { useTypedField } from "../composer/typed-field";
import { promptOfEvent } from "../attention/prompt";
import { NeedsSwitch, useLetsPhone } from "../phone/everything";
import { ASK_PLACEHOLDER, type DispatcherBar, dispatcherBar, resultLine, sheetOpens } from "./bar";
import { dispatcherPrefs } from "./prefs";

const flows = new WeakMap<Services, DispatchFlow>();

/** The app's one conversation with the dispatcher, kept while you move between screens. */
const flowOf = (services: Services): DispatchFlow => {
	let flow = flows.get(services);
	if (!flow) {
		flow = createDispatchFlow(services.source);
		flows.set(services, flow);
	}
	return flow;
};

// The swipe rows' spring, so the sheet moves as they do.
const SPRING = { damping: 26, mass: 0.7, stiffness: 330, overshootClamping: true };
// How much of the list shows over the open sheet.
const SHEET_TOP = 28;

// The sheet's slide, kept outside React as the swipe rows' is: how far up it
// is from 0 to 1, whether it's open, and the drag that moves it.
const createSlide = () => {
	const up = new Animated.Value(0);
	let open = false;
	let height = 0;
	let reduce = false;
	let from = 0;
	let settle: (open: boolean) => void = () => {};
	let show: (live: boolean) => void = () => {};
	const go = (to: boolean) => {
		open = to;
		if (to) show(true);
		const done = () => {
			if (!open) show(false);
		};
		if (reduce) {
			up.setValue(to ? 1 : 0);
			done();
			return;
		}
		Animated.spring(up, { toValue: to ? 1 : 0, useNativeDriver: true, ...SPRING }).start(
			({ finished }) => {
				if (finished) done();
			}
		);
	};
	const pan = PanResponder.create({
		onMoveShouldSetPanResponder: (_, g) =>
			height > 0 && Math.abs(g.dy) > 8 && Math.abs(g.dy) > Math.abs(g.dx) * 1.5,
		onPanResponderGrant: () => {
			up.stopAnimation();
			from = open ? 1 : 0;
			show(true);
		},
		onPanResponderMove: (_, g) => up.setValue(Math.min(1, Math.max(0, from - g.dy / height))),
		onPanResponderRelease: (_, g) => {
			const to = sheetOpens(open, g.dy, g.vy, height);
			go(to);
			settle(to);
		},
		onPanResponderTerminate: () => go(open),
	});
	return {
		up,
		pan,
		go,
		isOpen: () => open,
		set: (to: {
			height: number;
			reduce: boolean;
			settle: (open: boolean) => void;
			show: (live: boolean) => void;
		}) => {
			({ height, reduce, settle, show } = to);
		},
	};
};

/**
 * The list in `children`, with the sheet over it and the bar under it.
 * `onOpenAgent` opens the screen of an agent a plan started.
 */
export const Dispatcher = ({
	children,
	onOpenAgent,
}: {
	children: ReactNode;
	onOpenAgent: (agent: AgentId) => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const flow = flowOf(services);
	const state = useStore(flow.store);
	const prefs = useStore(dispatcherPrefs);
	const view = useView((s) => s.view);
	const [open, setOpen] = useState(false);
	// Whether the sheet is on screen: open, dragged, or sliding shut.
	const [live, setLive] = useState(false);
	const [height, setHeight] = useState(0);
	const reduce = useReduceMotion();
	const [slide] = useState(createSlide);
	useEffect(
		() => slide.set({ height, reduce, settle: setOpen, show: setLive }),
		[slide, height, reduce]
	);
	const bar = dispatcherBar(view, state, prefs, open);
	const insets = useSafeAreaInsets();

	useEffect(() => {
		if (slide.isOpen() !== open) slide.go(open);
	}, [slide, open]);

	// Android's back closes the sheet first.
	useBackPress(open, () => setOpen(false));

	const send = () => {
		if (!bar.canSend) return;
		Keyboard.dismiss();
		setOpen(true);
		const { device } = bar.reach;
		const pills = device !== null && takesAnswerPills(view, device);
		void ask(flow, { device, account: bar.reach.account?.id ?? null, pills });
	};

	const openAgent = (agent: AgentId) => {
		setOpen(false);
		onOpenAgent(agent);
	};

	const translateY = useMemo(
		() => slide.up.interpolate({ inputRange: [0, 1], outputRange: [height, 0] }),
		[slide, height]
	);

	return (
		<>
			<View style={styles.body} onLayout={(e) => setHeight(e.nativeEvent.layout.height)}>
				{children}
				{live ? (
					<>
						<Animated.View
							style={[
								StyleSheet.absoluteFill,
								styles.scrim,
								{ opacity: slide.up, pointerEvents: open ? "auto" : "none" },
							]}
						>
							<Pressable
								accessibilityLabel={t("Close the dispatcher")}
								onPress={() => setOpen(false)}
								style={StyleSheet.absoluteFill}
							/>
						</Animated.View>
						<Animated.View
							accessibilityLabel={t("Dispatcher")}
							style={[styles.sheet, { transform: [{ translateY }] }]}
						>
							<Sheet
								flow={flow}
								bar={bar}
								handle={slide.pan.panHandlers}
								onOpenAgent={openAgent}
							/>
						</Animated.View>
					</>
				) : null}
			</View>
			<View
				style={[styles.foot, { paddingBottom: insets.bottom }]}
				{...slide.pan.panHandlers}
			>
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={open ? t("Close the dispatcher") : t("Open the dispatcher")}
					accessibilityState={{ expanded: open }}
					onPress={() => setOpen((o) => !o)}
					style={styles.grab}
				>
					<View style={styles.handle} />
					{!open && bar.status ? (
						<Text style={styles.status} numberOfLines={1}>
							{bar.status}
						</Text>
					) : null}
				</Pressable>
				{bar.shows === "bar" ? (
					<Box flow={flow} text={state.text} refs={state.refs} bar={bar} onSend={send} />
				) : null}
			</View>
		</>
	);
};

/** A line of the box, and the space above and below its text. */
const LINE_HEIGHT = 20;
const FIELD_PADDING = 8;

/**
 * The box to ask in, or why the dispatcher can't take a request now. While
 * you type in it, it shows the whole draft and grows to a few lines before
 * it scrolls; otherwise it keeps to one line. Return sends.
 */
const Box = ({
	flow,
	text,
	refs,
	bar,
	onSend,
}: {
	flow: DispatchFlow;
	text: string;
	refs: Typed["refs"];
	bar: DispatcherBar;
	onSend: () => void;
}) => {
	const { t } = useI18n();
	// `@` names an agent, as in the desktop's box.
	const input = useRef<TextInputInstance>(null);
	const field = useTypedField({
		ref: input,
		text,
		refs,
		onChange: (typed) => setDraft(flow, typed.text, typed.refs),
		place: ANY_PLACE,
		device: bar.reach.device,
	});

	const { off } = bar.reach;
	const [focused, setFocused] = useState(false);
	const reduce = useReduceMotion();
	const focus = (on: boolean) => {
		if (!reduce) LayoutAnimation.configureNext(LayoutAnimation.Presets.easeInEaseOut);
		setFocused(on);
	};
	return (
		<>
			{field.menu}
			<View
				style={[styles.box, focused && styles.boxFocused]}
				accessibilityState={{ disabled: off !== null }}
			>
				{off ? (
					<Text style={styles.off} numberOfLines={2}>
						{off}
					</Text>
				) : (
					<TextInput
						ref={input}
						style={[styles.input, focused ? styles.inputOpen : styles.inputShut]}
						onChangeText={field.input.onChangeText}
						onSelectionChange={field.input.onSelectionChange}
						onSubmitEditing={onSend}
						onFocus={() => focus(true)}
						onBlur={() => focus(false)}
						multiline
						scrollEnabled={focused}
						submitBehavior="submit"
						returnKeyType="send"
						spellCheck
						autoCorrect
						placeholder={t(ASK_PLACEHOLDER)}
						placeholderTextColor={color.subtleForeground}
						accessibilityLabel={t("Ask the dispatcher")}
					>
						{field.input.children}
					</TextInput>
				)}
				<Button
					label={t("Send")}
					tone="primary"
					small
					disabled={!bar.canSend}
					onPress={onSend}
				/>
			</View>
		</>
	);
};

/** The sheet: its title, the conversation, and the plan or how it went. */
const Sheet = ({
	flow,
	bar,
	handle,
	onOpenAgent,
}: {
	flow: DispatchFlow;
	bar: DispatcherBar;
	handle: ReturnType<typeof createSlide>["pan"]["panHandlers"];
	onOpenAgent: (agent: AgentId) => void;
}) => {
	const { t } = useI18n();

	const { busy, note, ran } = useStore(flow.store);
	const { talking, plan, reach } = bar;
	const device = useView((s) => (reach.device ? deviceName(s.view, reach.device) : t("the Mac")));
	const scroller = useRef<ComponentRef<typeof ScrollView>>(null);
	const blocked = busy || reach.off !== null;
	// The Mac's maxd refuses a phone's confirm of Auto or Full access
	// unless it lets this phone do everything.
	const lets = useLetsPhone(reach.device);
	const held = !lets && !!plan && plan.items.some((item) => stepNeedsSwitch(item.step));

	return (
		<>
			<View {...handle} style={styles.top}>
				<View style={styles.handle} />
				<View style={styles.head}>
					<Text style={styles.title} numberOfLines={1}>
						{dispatcherTitle(reach.account)}
					</Text>
					<Button
						label={t("New conversation")}
						tone="link"
						small
						disabled={busy || talking === undefined}
						onPress={() => startOver(flow)}
					/>
				</View>
			</View>
			<ScrollView
				ref={scroller}
				contentContainerStyle={styles.content}
				onContentSizeChange={() => scroller.current?.scrollToEnd({ animated: false })}
				keyboardShouldPersistTaps="handled"
			>
				{talking ? (
					<Conversation agent={talking} device={device} />
				) : (
					<Text style={styles.hint}>{noConversationHint(busy)}</Text>
				)}
				{ran ? (
					<Results run={ran} onOpenAgent={onOpenAgent} />
				) : plan ? (
					<PlanItems key={plan.id} plan={plan} lets={lets} />
				) : null}
				{note ? (
					<Text accessibilityRole="alert" style={styles.note}>
						{note}
					</Text>
				) : null}
			</ScrollView>
			{!ran && plan ? (
				<View style={styles.actions}>
					<Text style={styles.confirmHint}>{t(CONFIRM_HINT)}</Text>
					<Button
						label={t("Dismiss")}
						small
						disabled={blocked}
						onPress={() => void dismiss(flow, reach.device, plan)}
					/>
					<Button
						label={t("Confirm")}
						tone="primary"
						small
						disabled={blocked || held}
						onPress={() => void confirm(flow, reach.device, plan)}
					/>
				</View>
			) : null}
			{!ran && held ? (
				<View style={styles.switchLine}>
					<NeedsSwitch />
				</View>
			) : null}
		</>
	);
};

const toggled = (set: ReadonlySet<number>, index: number): Set<number> => {
	const next = new Set(set);
	if (!next.delete(index)) next.add(index);
	return next;
};

/**
 * What was said and asked, with each question answered in place. Its
 * reading and tool calls stay on the agent's own screen.
 */
const Conversation = ({ agent, device }: { agent: Agent; device: string }) => {
	const { source } = useServices();
	const timeline = useTimeline(source, agent.id);
	const rows = useFeedRows(timeline.store, inConversation);
	const asking = useView((s) => attentionOf(s.view, agent));
	// The question it waits on, when the feed has no card for it yet.
	const pending = asking && feedIndexOf(rows, asking.id) < 0 ? asking : undefined;
	return (
		<FeedProvider agent={agent} device={device} store={timeline.store}>
			{/* One block in the sheet, whose own gap would add to the rows' spacing. */}
			<View>
				{rows.map((row) => (
					<FeedItem key={row.id} id={row.id} gap={row.gap} />
				))}
				{pending ? (
					<View style={styles.pending}>
						<AttentionCard
							prompt={promptOfEvent(pending)}
							vendor={agent.vendor}
							place={device}
						/>
					</View>
				) : null}
				{isWorking(agent) ? <Text style={styles.waiting}>{t(WORKING_HINT)}</Text> : null}
			</View>
		</FeedProvider>
	);
};

/**
 * A plan waiting on you: its summary, and its items, each opening to its
 * lines. Unless the Mac `lets` this phone do everything, an item that
 * names Auto or Full access says it needs the switch.
 */
const PlanItems = ({ plan, lets }: { plan: Plan; lets: boolean }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const [opened, setOpened] = useState<ReadonlySet<number>>(() => new Set());
	const onToggle = (index: number) => setOpened(toggled(opened, index));
	return (
		<View style={styles.section} accessibilityLabel={t("Plan")}>
			<Text style={styles.sectionTitle}>{sentence(planSummary(view, plan))}</Text>
			{plan.items.map((item, index) => {
				const open = opened.has(index);
				return (
					<View key={index} style={styles.item}>
						<Pressable
							accessibilityRole="button"
							accessibilityState={{ expanded: open }}
							onPress={() => onToggle(index)}
							style={styles.itemHead}
						>
							<View style={styles.chevron}>
								<Chevron dir={open ? "down" : "right"} size={12} />
							</View>
							<View style={styles.itemText}>
								<Text style={styles.phrase}>
									{sentence(itemPhrase(view, item))}
								</Text>
								<Text style={styles.reason}>{item.reason}</Text>
								{!lets && stepNeedsSwitch(item.step) ? (
									<Text style={styles.reason}>{t("Needs the switch")}</Text>
								) : null}
							</View>
						</Pressable>
						{open ? (
							<View style={styles.lines}>
								{itemLines(view, plan, index).map((line, i) => (
									<View key={`${i}:${line.label}`} style={styles.line}>
										<Text style={styles.lineLabel}>{line.label}</Text>
										<Text selectable style={styles.lineValue}>
											{line.value}
										</Text>
									</View>
								))}
							</View>
						) : null}
					</View>
				);
			})}
		</View>
	);
};

/** A plan that ran: how each item went. A tap on an agent it started opens it. */
const Results = ({ run, onOpenAgent }: { run: PlanRun; onOpenAgent: (agent: AgentId) => void }) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const rows = resultRows(view, run.plan, run.results);
	return (
		<View style={styles.section} accessibilityLabel={t("Plan results")}>
			<Text style={styles.sectionTitle}>{resultsTitle(run.results)}</Text>
			{rows.map((row, index) => {
				const { agent } = row;
				const body = (
					<>
						<View style={styles.itemText}>
							<Text style={styles.phrase}>{row.phrase}</Text>
							<Text style={[styles.reason, row.failed && styles.failed]}>
								{resultLine(row)}
							</Text>
						</View>
						{agent ? <Chevron dir="right" size={12} /> : null}
					</>
				);
				return agent ? (
					<Pressable
						key={index}
						accessibilityRole="link"
						accessibilityHint={t("Opens the agent")}
						onPress={() => onOpenAgent(agent)}
						style={({ pressed }) => [
							styles.item,
							styles.result,
							pressed && styles.pressed,
						]}
					>
						{body}
					</Pressable>
				) : (
					<View key={index} style={[styles.item, styles.result]}>
						{body}
					</View>
				);
			})}
		</View>
	);
};

const styles = StyleSheet.create({
	body: { flex: 1, overflow: "hidden" },
	scrim: { backgroundColor: tint(color.canvas, 0.6) },
	sheet: {
		position: "absolute",
		top: SHEET_TOP,
		left: 0,
		right: 0,
		bottom: 0,
		borderTopLeftRadius: 14,
		borderTopRightRadius: 14,
		borderWidth: StyleSheet.hairlineWidth,
		borderBottomWidth: 0,
		borderColor: color.input,
		backgroundColor: color.card,
	},
	top: { paddingTop: 8, paddingBottom: 4, gap: 4 },
	head: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingLeft: 14,
		paddingRight: 4,
	},
	title: { flex: 1, color: color.mutedForeground, fontSize: size.meta },
	content: { paddingHorizontal: 14, paddingTop: 6, paddingBottom: 14, gap: 12 },
	pending: { marginTop: 12 },
	hint: { color: color.mutedForeground, fontSize: size.meta },
	waiting: { marginTop: 12, color: color.mutedForeground, fontSize: size.meta },
	note: { color: color.errorText, fontSize: size.meta },
	section: { gap: 6 },
	sectionTitle: { color: color.foreground, fontSize: size.small, fontWeight: "600" },
	item: {
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.track,
		backgroundColor: color.subpanel,
	},
	itemHead: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "flex-start",
		gap: 8,
		paddingHorizontal: 10,
		paddingVertical: 9,
	},
	chevron: { paddingTop: 3 },
	itemText: { flex: 1, minWidth: 0, gap: 2 },
	phrase: { color: color.foreground, fontSize: size.small },
	reason: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText },
	lines: {
		gap: 6,
		paddingHorizontal: 10,
		paddingVertical: 9,
		borderTopWidth: StyleSheet.hairlineWidth,
		borderTopColor: color.border,
	},
	line: { gap: 1 },
	lineLabel: { color: color.mutedForeground, fontSize: size.tiny },
	lineValue: { color: color.foreground, fontSize: size.meta },
	result: {
		minHeight: TOUCH,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 10,
		paddingVertical: 9,
	},
	pressed: { backgroundColor: color.accent },
	actions: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 14,
		paddingVertical: 8,
		borderTopWidth: StyleSheet.hairlineWidth,
		borderTopColor: color.border,
	},
	confirmHint: { flex: 1, color: color.mutedForeground, fontSize: size.tiny },
	switchLine: { paddingHorizontal: 14, paddingBottom: 8 },
	foot: {
		backgroundColor: color.background,
		borderTopWidth: StyleSheet.hairlineWidth,
		borderTopColor: color.border,
		paddingHorizontal: 10,
	},
	grab: {
		minHeight: 22,
		alignItems: "center",
		justifyContent: "center",
		gap: 3,
		paddingVertical: 6,
	},
	handle: {
		alignSelf: "center",
		width: 36,
		height: 4,
		borderRadius: 2,
		backgroundColor: color.input,
	},
	status: { color: color.mutedForeground, fontSize: size.tiny },
	box: {
		minHeight: 48,
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		marginBottom: 8,
		paddingLeft: 12,
		paddingRight: 6,
		borderRadius: 10,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.subpanel,
	},
	boxFocused: { borderColor: color.primary },
	input: {
		flex: 1,
		minHeight: LINE_HEIGHT + FIELD_PADDING * 2,
		paddingHorizontal: 0,
		paddingVertical: FIELD_PADDING,
		color: color.foreground,
		fontSize: size.body,
		lineHeight: LINE_HEIGHT,
		textAlignVertical: "top",
	},
	inputOpen: { maxHeight: fieldMaxHeight(FIELD_LINES.phone, LINE_HEIGHT, FIELD_PADDING) },
	inputShut: { maxHeight: LINE_HEIGHT + FIELD_PADDING * 2 },
	off: { flex: 1, color: color.mutedForeground, fontSize: size.small },
});
