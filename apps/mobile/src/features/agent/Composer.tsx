// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { useI18n } from "@maxhq/shared/i18n";

import {
	useCallback,
	useEffect,
	useEffectEvent,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import { Pressable, StyleSheet, Text, TextInput, type TextInputInstance, View } from "react-native";
import { useStore } from "zustand";

import { keptValue, useKept } from "@maxhq/shared/agent-view/kept";
import { agentDevice } from "@maxhq/shared/agent-view/move-agent";
import type { Agent, AgentId, DeviceId, SendMode, TimelineItem } from "@maxhq/shared/bindings";
import { imageNote } from "@maxhq/shared/composer/images";
import {
	attempt,
	type ComposerAction,
	failureOf,
	nowMode,
	placeholderFor,
	type QueuedMessage,
	turnOf,
	waitingMessages,
} from "@maxhq/shared/composer/send";
import { factsOfStaged, type StagedFile } from "@maxhq/shared/composer/staging";
import type { ActionResult } from "@maxhq/shared/data/source";
import { agentPillOf } from "@maxhq/shared/pills/agent-pill";
import { fileNumbers, markedText, type Segment } from "@maxhq/shared/pills/draft";
import { shownItems } from "@maxhq/shared/pills/git-pills";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import type { TriggerKind } from "@maxhq/shared/pills/trigger";
import { agreeToSend } from "@maxhq/shared/settings/auto-mode";
import { notePillBlocked } from "@maxhq/shared/view/notes";
import { modelsOf } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, size, TOUCH } from "../../theme";
import { Button } from "../../ui/Button";
import { choose } from "../../ui/choose";
import { Paperclip } from "../../ui/glyphs";
import { pickFiles, type PickFrom } from "../composer/attach";
import { AttachStrip } from "../composer/AttachStrip";
import { type Box, boxOf } from "../composer/box";
import {
	draftKeys,
	draftOfFragment,
	EMPTY_DRAFT,
	filled,
	fragmentOf,
	type PhoneDraft,
	queuedDraft,
	queuedFiles,
	queuedInput,
	wholeOf,
	withRestored,
} from "../composer/draft";
import { openBox } from "../composer/open-boxes";
import { usePillField } from "../composer/PillField";
import { sendDraft } from "../composer/send";
import { askAutoDefault, autoConsent } from "./auto-consent";
import { canSend, draftKey, otherSend, sendLabel, sendMode, stopsTurn } from "./compose";
import { QueuedList } from "./Queued";
import { savedDrafts } from "./saved-drafts";
import { busySendPref } from "./send-pref";

/** The words that open a menu in the box, as on the desktop. */
const KINDS: readonly TriggerKind[] = ["path", "git-item", "skill"];

const CLIP = 32;

/** The draft the phone saved for `agent`, with its files and pull requests back in `box`. */
const savedDraft = (box: Box, agent: AgentId): PhoneDraft => {
	const fragment = savedDrafts.get(agent);
	if (!fragment) return EMPTY_DRAFT;
	const restored = draftOfFragment(fragment);
	box.staging.getState().restore(fragment.files);
	box.gitPills.getState().seed(restored.gitItems);
	return restored.draft;
};

/**
 * The box under an agent's output, bordered as the desktop's, with the send
 * button inside it and the messages that wait for the turn above it. Once
 * you type, a second button beside it steers or queues, whichever the send
 * button doesn't, while a turn runs, and skips the prompt while one waits.
 * The paperclip adds photos and files, which show in a strip above the
 * words, and `@`, `#` and `$` put in pills as the desktop's box does. What
 * you write stays with the agent until it goes, pills and files too, and
 * the phone saves it, so it comes back after a restart.
 */
export const Composer = ({
	agent,
	device,
	queued,
}: {
	agent: Agent;
	device: string;
	/** The agent's messages that wait for the turn to end. */
	queued: readonly TimelineItem[];
}) => {
	const { t } = useI18n();

	const services = useServices();
	const { source } = services;
	const view = useView((state) => state.view);
	// Where the agent runs, where its files stage and its pills point.
	const home = agentDevice(view, agent);
	const on = home ?? ("" as DeviceId);
	const box = useMemo(() => boxOf(source, agent.id, on), [agent.id, on, source]);
	const { staging, gitPills } = box;
	// What you wrote while the app ran, or else what the phone saved before.
	const [first] = useState(
		() => keptValue<PhoneDraft | null>(draftKey(agent.id), null) ?? savedDraft(box, agent.id)
	);
	const [draft, keepDraft] = useKept<PhoneDraft>(draftKey(agent.id), first);
	const files = useStore(staging, (state) => state.files);
	const gitState = useStore(gitPills);
	// The draft as the last change left it, for a pick that ends after it.
	const latest = useRef(draft);
	useLayoutEffect(() => {
		latest.current = draft;
	}, [draft]);
	const setDraft = (next: PhoneDraft) => {
		latest.current = next;
		keepDraft(next);
		staging.getState().keepOnly(draftKeys(next));
	};

	// The phone saves the draft as it changes, with each file once it's ready.
	useEffect(() => {
		savedDrafts.save(
			agent.id,
			fragmentOf(draft, files, on, shownItems(gitState, markedText(wholeOf(draft))))
		);
	}, [agent.id, draft, files, gitState, on]);

	const [sending, setSending] = useState(false);
	const [waitingFiles, setWaitingFiles] = useState(false);
	const [error, setError] = useState<string | null>(null);
	// Messages taken back, until the timeline drops them, and those on their way back.
	const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
	const [pending, setPending] = useState<ReadonlySet<string>>(() => new Set());
	const waiting = useMemo(() => waitingMessages(queued, hidden), [queued, hidden]);
	const open = canSend(agent.activity);
	const pick = useStore(busySendPref, (s) => s.pick);
	const full = filled(draft);
	const stop = stopsTurn(agent.activity, full);
	const other = otherSend(agent.activity, pick, full);
	const { label, hint } = stop
		? { label: t("Stop"), hint: null }
		: sendLabel(agent.activity, pick);

	const ownProject = view.tables.workspace.get(agent.workspace)?.project ?? null;
	const factsOf = (key: string) => {
		const file = staging.getState().files.find((each) => each.key === key);
		return file && factsOfStaged(file);
	};
	const inputRef = useRef<TextInputInstance>(null);
	const field = usePillField({
		ref: inputRef,
		segments: draft.segments,
		onChange: (segments) => setDraft({ segments, loose: latest.current.loose }),
		factsOf,
		gitPills,
		agent: agent.id,
		kinds: KINDS,
		onPath:
			home === null
				? null
				: (path): Segment[] => [
						{
							type: "pill",
							pill: {
								type: "attachment",
								key: staging.getState().addFound(agent.id, home, path),
							},
						},
					],
		agentsOn: null,
		notes: notePillBlocked(view, on) === null ? { own: ownProject, only: false } : null,
		agentPill: (other) => agentPillOf(view, other, on),
	});

	// Quotes sent here from a message, now and later, go in at the caret.
	const takeQuotes = useEffectEvent(() => {
		const taken = quoteInbox.take(agent.id);
		const quotes = taken.flatMap(({ quote }) => (quote ? [quote] : []));
		if (quotes.length === 0) return;
		field.insert(
			quotes.flatMap((quote, index): Segment[] => [
				...(index > 0 ? [{ type: "text", text: " " } as const] : []),
				{ type: "pill", pill: quote },
			])
		);
		if (taken.some(({ focus }) => focus)) inputRef.current?.focus();
	});
	useEffect(() => {
		takeQuotes();
		const unwatch = quoteInbox.watch(agent.id, () => takeQuotes());
		// Replies offer Quote while this box shows.
		const shut = openBox(agent.id);
		return () => {
			unwatch();
			shut();
		};
	}, [agent.id]);

	/** Runs an action and shows why it failed, if it did. Returns whether it went through. */
	const run = async (
		what: ComposerAction,
		action: () => Promise<ActionResult>
	): Promise<boolean> => {
		setError(null);
		const failure = failureOf(await attempt(action), what);
		setError(failure);
		return failure === null;
	};

	const press = async (mode: SendMode) => {
		if (sending) return;
		setSending(true);
		try {
			if (stop) {
				await run("stop", () => source.interrupt(agent.id));
				return;
			}
			const now = latest.current;
			if (!filled(now)) return;
			setError(null);
			const viewNow = () => services.view.getState().view;
			const { sent, failure } = await sendDraft(
				{
					source,
					view: viewNow,
					agent: agent.id,
					device: on,
					ownProject,
					staging,
					gitPills,
					// An Auto you didn't pick, as the device's default, asks first.
					agree: () => agreeToSend(agent, autoConsent, askAutoDefault, source, viewNow()),
					onWait: setWaitingFiles,
				},
				now,
				mode
			);
			if (sent) {
				setDraft(EMPTY_DRAFT);
				field.shutAt([]);
			}
			setError(failure);
		} finally {
			setSending(false);
		}
	};

	const attach = async (from: PickFrom) => {
		const picked = await pickFiles(from);
		if ("failure" in picked) {
			setError(picked.failure);
			return;
		}
		const added: string[] = [];
		let refused: string | null = null;
		for (const file of picked.files) {
			const result = staging.getState().addPicked(file);
			if ("key" in result) added.push(result.key);
			else refused = result.refused;
		}
		setError(refused);
		if (added.length === 0) return;
		const now = latest.current;
		setDraft({ ...now, loose: [...now.loose, ...added] });
	};

	const openAttach = () =>
		choose(t("Add a photo or file"), [
			{ label: t("Photo library"), pick: () => void attach("library") },
			{ label: t("Take photo"), pick: () => void attach("camera") },
			{ label: t("Choose file"), pick: () => void attach("files") },
		]);

	// The strip's X keeps one callback, so its files draw again only when they change.
	const removeFile = useCallback(
		(key: string) => {
			const now = latest.current;
			const next = { ...now, loose: now.loose.filter((each) => each !== key) };
			latest.current = next;
			keepDraft(next);
			staging.getState().keepOnly(draftKeys(next));
		},
		[keepDraft, staging]
	);

	const mark = (id: string, on: boolean, set: typeof setPending) =>
		set((current) => {
			const next = new Set(current);
			if (on) next.add(id);
			else next.delete(id);
			return next;
		});

	const remove = async (message: QueuedMessage) => {
		mark(message.id, true, setPending);
		if (await run("unqueue", () => source.cancelQueued(agent.id, message.id)))
			mark(message.id, true, setHidden);
		mark(message.id, false, setPending);
	};

	/** Puts a message the queue gave back in the box, before anything already there. */
	const restore = (message: QueuedMessage) => {
		const keyOf = new Map(
			queuedFiles(message).map((ref) => [ref.id, staging.getState().addReady(ref)])
		);
		const restored = queuedDraft(message, (id) => keyOf.get(id) ?? id);
		gitPills.getState().seed(restored.gitItems);
		const next = withRestored(restored.draft, latest.current);
		setDraft(next);
		field.shutAt(next.segments);
	};

	const edit = async (message: QueuedMessage) => {
		mark(message.id, true, setHidden);
		if (await run("unqueue", () => source.cancelQueued(agent.id, message.id))) restore(message);
		else mark(message.id, false, setHidden);
	};

	/** Steers a queued message into the turn, or sends it now past the prompts that wait. */
	const steerNow = async (message: QueuedMessage) => {
		const mode = nowMode(turnOf(agent.activity));
		if (mode === null) return;
		const action = mode === "steer" ? "steer" : "send";
		mark(message.id, true, setPending);
		if (await run(action, () => source.cancelQueued(agent.id, message.id))) {
			mark(message.id, true, setHidden);
			const steered = await run(action, () =>
				source.sendInput(agent.id, queuedInput(message), mode)
			);
			// Keep the words, since the queue no longer has them.
			if (!steered) restore(message);
		}
		mark(message.id, false, setPending);
	};

	const whole = wholeOf(draft);
	const numbers = fileNumbers(whole, factsOf);
	const strip = draft.loose.flatMap((key): StagedFile[] => {
		const file = files.find((each) => each.key === key);
		return file ? [file] : [];
	});
	const account = agent.account ? view.tables.account.get(agent.account) : undefined;
	const note = files.some((file) => (file.ref?.kind ?? file.kind) === "image")
		? imageNote(account, agent.model, modelsOf(view, account))
		: null;
	const busy = !open || sending;

	return (
		<View style={styles.bar}>
			<QueuedList
				messages={waiting}
				now={nowMode(turnOf(agent.activity))}
				pending={pending}
				onSteer={(message) => void steerNow(message)}
				onEdit={(message) => void edit(message)}
				onRemove={(message) => void remove(message)}
			/>
			{error ? <Text style={styles.failed}>{error}</Text> : null}
			{field.menu}
			<View style={styles.box}>
				<AttachStrip
					files={strip}
					numbers={numbers}
					disabled={sending}
					onRemove={removeFile}
				/>
				{note ? <Text style={styles.note}>{note}</Text> : null}
				<TextInput
					ref={inputRef}
					style={styles.input}
					onChangeText={field.input.onChangeText}
					onSelectionChange={field.input.onSelectionChange}
					editable={!busy}
					multiline
					placeholder={placeholderFor(agent.activity, agent.title, device)}
					placeholderTextColor={color.subtleForeground}
					accessibilityLabel={t("Message")}
				>
					{field.input.children}
				</TextInput>
				<View style={styles.row}>
					<Pressable
						accessibilityRole="button"
						accessibilityLabel={t("Attach")}
						accessibilityState={{ disabled: busy || home === null }}
						disabled={busy || home === null}
						onPress={openAttach}
						hitSlop={(TOUCH - CLIP) / 2}
						style={({ pressed }) => [
							styles.clip,
							pressed && styles.pressed,
							(busy || home === null) && styles.off,
						]}
					>
						<Paperclip size={18} strokeWidth={1.8} color={color.mutedForeground} />
					</Pressable>
					<Text style={styles.hint} numberOfLines={1}>
						{waitingFiles ? t("Sends when the files are ready") : (hint ?? "")}
					</Text>
					{other ? (
						<Button
							label={other.label}
							hint={other.hint}
							small
							disabled={busy}
							onPress={() => void press(other.mode)}
						/>
					) : null}
					<Button
						label={label}
						tone={stop ? "secondary" : "primary"}
						small
						disabled={busy || (!stop && !full)}
						onPress={() => void press(sendMode(agent.activity, pick))}
					/>
				</View>
			</View>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: { backgroundColor: color.background, paddingHorizontal: 10, paddingVertical: 8, gap: 6 },
	box: {
		borderColor: color.input,
		borderWidth: StyleSheet.hairlineWidth,
		borderRadius: 10,
		backgroundColor: color.subpanel,
		paddingHorizontal: 12,
		paddingTop: 10,
		paddingBottom: 8,
		gap: 6,
	},
	input: {
		minHeight: 36,
		maxHeight: 120,
		padding: 0,
		color: color.foreground,
		fontSize: size.body,
		lineHeight: 20,
	},
	row: { flexDirection: "row", alignItems: "center", gap: 8 },
	clip: {
		width: CLIP,
		height: CLIP,
		marginLeft: -6,
		borderRadius: CLIP / 2,
		alignItems: "center",
		justifyContent: "center",
	},
	pressed: { backgroundColor: color.accent },
	off: { opacity: 0.4 },
	hint: { flex: 1, color: color.subtleForeground, fontSize: size.tiny },
	note: { color: color.mutedForeground, fontSize: size.tiny },
	failed: { color: color.errorText, fontSize: size.meta },
});
