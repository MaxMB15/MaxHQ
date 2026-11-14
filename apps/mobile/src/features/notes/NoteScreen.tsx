// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// One project note. It reads as Markdown, and Edit turns it into a title
// field and a text box with the Markdown source. Edits save as you type,
// by the same rules as the desktop's pane in `@maxhq/shared/notes/autosave`:
// a second after you stop, at most ten seconds apart, and right away when
// a field loses focus, the screen goes or the app leaves the screen. Text
// from elsewhere, a merge or another device's save, comes into the box
// around what you typed. The menu deletes the note. A note deleted
// elsewhere says so and offers it back.

import { useI18n } from "@maxhq/shared/i18n";

import { useEffect, useRef, useState, useSyncExternalStore, type ComponentRef } from "react";
import {
	Alert,
	AppState,
	KeyboardAvoidingView,
	Platform,
	ScrollView,
	StyleSheet,
	Text,
	TextInput,
	View,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { DeviceId, NoteId, NoteVersion, ProjectId } from "@maxhq/shared/bindings";
import { deleteNoteQuestion } from "@maxhq/shared/notes/actions";
import { createAutosave, incomingText, saveText } from "@maxhq/shared/notes/autosave";
import { errorText } from "@maxhq/shared/timeline/store";
import { noteOn, remoteNotesReach, shownTitle } from "@maxhq/shared/view/notes";

import type { ScreenProps } from "../../navigation";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose, confirm } from "../../ui/choose";
import { Header, IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { MarkdownText } from "../../ui/MarkdownText";

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

	const changeBody = (text: string) => {
		typed.current = text;
		setBody(text);
		saving.edit({ body: text });
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

	return (
		<KeyboardAvoidingView
			style={styles.screen}
			behavior={Platform.OS === "ios" ? "padding" : undefined}
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
							<MarkdownText text={body} />
						)}
					</>
				)}
			</ScrollView>
		</KeyboardAvoidingView>
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
		fontFamily: Platform.select({ ios: "Menlo", default: "monospace" }),
		padding: 0,
	},
	empty: { color: color.mutedForeground, fontSize: size.small },
	message: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
	gone: { alignItems: "center", gap: 12, paddingTop: 48 },
});
