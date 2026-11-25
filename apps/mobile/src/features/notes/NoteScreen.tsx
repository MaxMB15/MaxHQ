// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One project note. It reads as Markdown, and Edit turns it into a title
// field and a text box with the Markdown source. Edits save as you type,
// by the same rules as the desktop's pane in `@maxhq/shared/notes/autosave`:
// a second after you stop, at most ten seconds apart, and right away when
// a field loses focus, the screen goes or the app leaves the screen. Text
// from elsewhere, a merge or another device's save, comes into the box
// around what you typed. A tap on a task box in the rendered note ticks it
// and saves the same way. The menu opens the note's history, where an old
// version reads as it was and Restore saves it as the newest, or deletes
// the note. A note deleted elsewhere says so and offers it back.

import { useI18n } from "@maxhq/shared/i18n";

import {
	useCallback,
	useEffect,
	useRef,
	useState,
	useSyncExternalStore,
	type ComponentRef,
} from "react";
import {
	Alert,
	AppState,
	FlatList,
	KeyboardAvoidingView,
	Pressable,
	ScrollView,
	StyleSheet,
	Text,
	TextInput,
	View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type {
	DeviceId,
	NoteId,
	NoteVersion,
	NoteVersionInfo,
	ProjectId,
	VersionHash,
} from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { deleteNoteQuestion } from "@maxhq/shared/notes/actions";
import { createAutosave, incomingText, saveText } from "@maxhq/shared/notes/autosave";
import { setTask } from "@maxhq/shared/notes/tasks";
import { errorText } from "@maxhq/shared/timeline/store";
import {
	noteOn,
	remoteNotesReach,
	savedBy,
	shownTitle,
	versionHeading,
	versionWhen,
} from "@maxhq/shared/view/notes";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { useBackPress } from "../../platform/back";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose, confirm } from "../../ui/choose";
import { Header, IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { MarkdownText } from "../../ui/MarkdownText";
import {
	type Loaded,
	pickVersion,
	readHistory,
	readVersion,
	type Showing,
	stepBack,
} from "./history";

type Load =
	| { readonly kind: "reading" }
	| { readonly kind: "failed"; readonly reason: string }
	| { readonly kind: "read"; readonly version: NoteVersion };

type Params = ScreenProps<"Note">["route"]["params"];

export const NoteScreen = ({ navigation, route }: ScreenProps<"Note">) => {
	useI18n();

	const { project } = route.params;
	const view = useView((s) => s.view);
	const reach = remoteNotesReach(view, project);
	const back = () => navigation.goBack();
	if (reach.device === null) return <Message text={reach.why} onBack={back} />;
	// Another device's copy is another note's history, so it opens anew.
	return (
		<NoteLoad key={reach.device} device={reach.device} params={route.params} onBack={back} />
	);
};

/** A header with Back, and a line of text under it. */
const Message = ({ text, onBack }: { text: string; onBack: () => void }) => {
	const { t } = useI18n();
	return (
		<View style={styles.screen}>
			<Header title={t("Note")} back={{ label: t("Back"), onPress: onBack }} />
			<Text style={styles.message}>{text}</Text>
		</View>
	);
};

/** Reads the note's head from `device`, then shows it. */
const NoteLoad = ({
	device,
	params,
	onBack,
}: {
	device: DeviceId;
	params: Params;
	onBack: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { project, note } = params;
	const [load, setLoad] = useState<Load>({ kind: "reading" });
	useEffect(() => {
		let live = true;
		void source.readNote(device, project, note, null).then((result) => {
			if (!live) return;
			setLoad(
				result.status === "ok"
					? { kind: "read", version: result.data }
					: { kind: "failed", reason: errorText(result.error) }
			);
		});
		return () => {
			live = false;
		};
	}, [source, device, project, note]);
	if (load.kind === "reading") return <Message text={t("Reading the note")} onBack={onBack} />;
	if (load.kind === "failed") return <Message text={load.reason} onBack={onBack} />;
	return (
		<NoteBody
			device={device}
			project={project}
			note={note}
			initial={load.version}
			made={params.made === true}
			onBack={onBack}
		/>
	);
};

/** An open note, from the version it opened at. */
const NoteBody = ({
	device,
	project,
	note,
	initial,
	made,
	onBack,
}: {
	device: DeviceId;
	project: ProjectId;
	note: NoteId;
	initial: NoteVersion;
	made: boolean;
	onBack: () => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const insets = useSafeAreaInsets();
	const [saving] = useState(() =>
		createAutosave({
			initial,
			save: (save) => source.saveNote(device, project, note, save),
			read: async (version) => {
				const result = await source.readNote(device, project, note, version);
				return result.status === "ok" ? result.data : null;
			},
		})
	);
	const state = useSyncExternalStore(saving.subscribe, saving.snapshot);
	// A note just made opens to edit, with its title ready to type.
	const [editing, setEditing] = useState(made);
	const [body, setBody] = useState(initial.body);
	const typed = useRef(initial.body);
	const titleField = useRef<ComponentRef<typeof TextInput>>(null);
	const [showing, setShowing] = useState<Showing>({ kind: "note" });
	const [history, setHistory] = useState<Loaded<NoteVersionInfo[]>>({ kind: "reading" });
	const [historyTry, setHistoryTry] = useState(0);
	const [restoring, setRestoring] = useState(false);

	const changeBody = useCallback(
		(text: string) => {
			typed.current = text;
			setBody(text);
			saving.edit({ body: text });
		},
		[saving]
	);

	// A tap on a task box ticks it in the text, which saves as typing does.
	const tick = useCallback(
		(index: number, done: boolean) => {
			const next = setTask(typed.current, index, done);
			if (next !== typed.current) changeBody(next);
		},
		[changeBody]
	);

	// The history reads when it opens, and again when the note gets a new
	// version, with the list it had showing until then.
	const listing = showing.kind !== "note";
	useEffect(() => {
		if (!listing) return;
		let live = true;
		void readHistory(source, device, project, note).then((next) => {
			if (live) setHistory(next);
		});
		return () => {
			live = false;
		};
	}, [listing, source, device, project, note, state.base, historyTry]);

	// Android's Back steps out of an old version and the history before it leaves the note.
	useBackPress(listing, () => setShowing(stepBack));

	const openHistory = () => {
		void saving.flush();
		setHistory((was) => (was.kind === "failed" ? { kind: "reading" } : was));
		setShowing({ kind: "history" });
	};

	const restore = (version: NoteVersion) => {
		setRestoring(true);
		void saving.replace({ title: version.title, body: version.body }).then((failure) => {
			setRestoring(false);
			if (failure === null) setShowing({ kind: "note" });
			else Alert.alert(t("Couldn't restore it."), failure);
		});
	};

	// Text from elsewhere goes into the box around what was typed since,
	// and back to the saving when the two differ.
	useEffect(
		() =>
			saving.follow((incoming, against) => {
				const next = incomingText(typed.current, incoming, against);
				typed.current = next;
				setBody(next);
				saving.edit({ body: next });
			}),
		[saving]
	);

	// What waits is saved when the screen goes, and when the app leaves the screen.
	useEffect(() => {
		const watch = AppState.addEventListener("change", (next) => {
			if (next !== "active") void saving.flush();
		});
		return () => {
			watch.remove();
			void saving.flush();
		};
	}, [saving]);

	// The device's head moving, by a save elsewhere or a sync, comes in.
	const head = useView((s) => noteOn(s.view, device, project, note)?.head);
	useEffect(() => {
		if (head) saving.headMoved(head);
	}, [head, saving]);

	useEffect(() => {
		if (made) titleField.current?.focus();
	}, [made]);

	const remove = () =>
		confirm(deleteNoteQuestion(shownTitle(state.title)), t("Delete"), () => {
			void saving.replace({ deleted: true }).then((failure) => {
				if (failure === null) onBack();
				else Alert.alert(t("Couldn't delete the note"), failure);
			});
		});

	const header = (
		<Header
			title={shownTitle(state.title)}
			subtitle={
				<Text
					accessibilityRole="text"
					accessibilityLiveRegion="polite"
					numberOfLines={1}
					style={[styles.status, state.save.kind === "failed" && styles.failed]}
				>
					{saveText(state.save, false)}
				</Text>
			}
			back={{ label: t("Back"), onPress: onBack }}
			right={
				state.deleted ? null : (
					<>
						<Button
							label={editing ? t("Done") : t("Edit")}
							tone="link"
							small
							onPress={() => {
								if (editing) void saving.flush();
								setEditing((on) => !on);
							}}
						/>
						<IconButton
							label={t("More")}
							onPress={() =>
								choose(shownTitle(state.title), [
									{ label: t("History"), pick: openHistory },
									{ label: t("Delete"), destructive: true, pick: remove },
								])
							}
						>
							<More />
						</IconButton>
					</>
				)
			}
		/>
	);

	if (state.deleted) {
		return (
			<View style={styles.screen}>
				{header}
				<View style={styles.gone}>
					<Text style={styles.message}>
						{t("“{{title}}” was deleted.", { title: shownTitle(state.title) })}
					</Text>
					<Button label={t("Bring it back")} onPress={() => void saving.replace({})} />
				</View>
			</View>
		);
	}

	if (showing.kind === "history") {
		return (
			<HistoryList
				title={state.title}
				history={history}
				onPick={setShowing}
				onRetry={() => {
					setHistory({ kind: "reading" });
					setHistoryTry((n) => n + 1);
				}}
				onBack={() => setShowing(stepBack)}
			/>
		);
	}

	if (showing.kind === "version") {
		return (
			<VersionShown
				key={showing.hash}
				device={device}
				project={project}
				note={note}
				hash={showing.hash}
				restoring={restoring}
				onBack={() => setShowing(stepBack)}
				onCurrent={() => setShowing({ kind: "note" })}
				onRestore={restore}
			/>
		);
	}

	return (
		<KeyboardAvoidingView
			style={styles.screen}
			// Padding on Android too: it draws edge to edge, so the keyboard no longer shrinks the window.
			behavior="padding"
		>
			{header}
			<ScrollView
				contentContainerStyle={[styles.content, { paddingBottom: insets.bottom + 24 }]}
				keyboardShouldPersistTaps="handled"
				keyboardDismissMode="interactive"
			>
				{editing ? (
					<>
						<TextInput
							ref={titleField}
							value={state.title}
							onChangeText={(title) => saving.edit({ title })}
							onBlur={() => void saving.flush()}
							placeholder={t("Title")}
							placeholderTextColor={color.subtleForeground}
							accessibilityLabel={t("Title")}
							returnKeyType="next"
							selectTextOnFocus={made}
							style={styles.title}
						/>
						<TextInput
							value={body}
							onChangeText={changeBody}
							onBlur={() => void saving.flush()}
							placeholder={t("Write in Markdown")}
							placeholderTextColor={color.subtleForeground}
							accessibilityLabel={t("Note")}
							multiline
							scrollEnabled={false}
							autoCapitalize="sentences"
							textAlignVertical="top"
							style={styles.source}
						/>
					</>
				) : (
					<>
						<Text style={styles.title} accessibilityRole="header">
							{shownTitle(state.title)}
						</Text>
						{body.trim() === "" ? (
							<Text style={styles.empty}>{t("Empty. Tap Edit to write.")}</Text>
						) : (
							<MarkdownText text={body} onTask={tick} />
						)}
					</>
				)}
			</ScrollView>
		</KeyboardAvoidingView>
	);
};

/**
 * A note's versions, newest first, with when each was saved and by whom.
 * Picking the newest goes back to the note, any other shows it.
 */
const HistoryList = ({
	title,
	history,
	onPick,
	onRetry,
	onBack,
}: {
	title: string;
	history: Loaded<NoteVersionInfo[]>;
	onPick: (showing: Showing) => void;
	onRetry: () => void;
	onBack: () => void;
}) => {
	const { t } = useI18n();

	const view = useView((s) => s.view);
	const now = useNow(30_000);
	const insets = useSafeAreaInsets();
	return (
		<View style={styles.screen}>
			<Header
				title={t("History")}
				subtitle={
					<Text numberOfLines={1} style={styles.status}>
						{shownTitle(title)}
					</Text>
				}
				back={{ label: t("Back"), onPress: onBack }}
			/>
			{history.kind === "read" ? (
				<FlatList
					data={history.data}
					keyExtractor={(version) => version.hash}
					contentContainerStyle={[styles.list, { paddingBottom: insets.bottom + 24 }]}
					renderItem={({ item, index }) => (
						<Pressable
							accessibilityRole="button"
							accessibilityState={{ selected: index === 0 }}
							onPress={() => onPick(pickVersion(index, item.hash))}
							style={({ pressed }) => [styles.row, pressed && styles.pressed]}
						>
							<Text style={styles.when} numberOfLines={1}>
								{versionWhen(item, index === 0, now)}
							</Text>
							<Text style={styles.by} numberOfLines={1}>
								{savedBy(view, item.by)}
							</Text>
						</Pressable>
					)}
				/>
			) : history.kind === "failed" ? (
				<View style={styles.gone}>
					<Text style={styles.failure}>{history.reason}</Text>
					<Button label={t("Try again")} onPress={onRetry} />
				</View>
			) : (
				<Text style={styles.message}>{t("Reading…")}</Text>
			)}
		</View>
	);
};

/** An old version of a note, read-only, with a way back to the note and Restore. */
const VersionShown = ({
	device,
	project,
	note,
	hash,
	restoring,
	onBack,
	onCurrent,
	onRestore,
}: {
	device: DeviceId;
	project: ProjectId;
	note: NoteId;
	hash: VersionHash;
	restoring: boolean;
	onBack: () => void;
	onCurrent: () => void;
	onRestore: (version: NoteVersion) => void;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const insets = useSafeAreaInsets();
	const now = useNow(30_000);
	const [version, setVersion] = useState<Loaded<NoteVersion>>({ kind: "reading" });
	const [attempt, setAttempt] = useState(0);
	useEffect(() => {
		let live = true;
		void readVersion(source, device, project, note, hash).then((next) => {
			if (live) setVersion(next);
		});
		return () => {
			live = false;
		};
	}, [source, device, project, note, hash, attempt]);

	const read = version.kind === "read" ? version.data : null;
	return (
		<View style={styles.screen}>
			<Header title={t("History")} back={{ label: t("Back"), onPress: onBack }} />
			<View style={styles.bar}>
				{version.kind === "failed" ? null : (
					<Text style={styles.heading} numberOfLines={2}>
						{read ? versionHeading(read, now) : t("Reading the version…")}
					</Text>
				)}
				<View style={styles.actions}>
					<Button label={t("Back to current")} small onPress={onCurrent} />
					<Button
						label={t("Restore")}
						small
						disabled={read === null || restoring}
						onPress={() => {
							if (read) onRestore(read);
						}}
					/>
				</View>
			</View>
			{version.kind === "failed" ? (
				<View style={styles.gone}>
					<Text style={styles.failure}>{version.reason}</Text>
					<Button
						label={t("Try again")}
						onPress={() => {
							setVersion({ kind: "reading" });
							setAttempt((n) => n + 1);
						}}
					/>
				</View>
			) : read ? (
				<ScrollView
					contentContainerStyle={[styles.content, { paddingBottom: insets.bottom + 24 }]}
				>
					{read.body.trim() === "" ? (
						<Text style={styles.empty}>{t("Empty")}</Text>
					) : (
						<MarkdownText text={read.body} />
					)}
				</ScrollView>
			) : null}
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { paddingHorizontal: 16, paddingTop: 16, gap: 12 },
	status: { color: color.mutedForeground, fontSize: size.meta },
	failed: { color: color.errorText },
	title: {
		color: color.foregroundStrong,
		fontSize: size.title,
		fontWeight: "700",
		padding: 0,
	},
	source: {
		minHeight: 240,
		color: color.foreground,
		fontSize: size.body,
		lineHeight: 20,
		fontFamily: font.mono,
		padding: 0,
	},
	empty: { color: color.mutedForeground, fontSize: size.small },
	message: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	gone: { alignItems: "center", gap: 12, paddingTop: 48 },
	failure: {
		color: color.errorText,
		fontSize: size.small,
		textAlign: "center",
		paddingHorizontal: 16,
	},
	list: { paddingHorizontal: 10, paddingTop: 8 },
	row: {
		minHeight: 52,
		justifyContent: "center",
		gap: 1,
		paddingHorizontal: 10,
		borderRadius: 6,
	},
	pressed: { backgroundColor: color.accent },
	when: { color: color.foregroundStrong, fontSize: size.body, fontVariant: ["tabular-nums"] },
	by: { color: color.mutedForeground, fontSize: size.meta },
	bar: {
		gap: 8,
		paddingHorizontal: 16,
		paddingVertical: 10,
		backgroundColor: color.subpanel,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	heading: { color: color.secondaryForeground, fontSize: size.small },
	actions: { flexDirection: "row", gap: 8 },
});
