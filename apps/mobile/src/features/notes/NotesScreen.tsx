// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's notes, as the desktop lists them under the project: each
// note's title, who changed it last and when. A tap opens the note, and a
// long press offers Delete, which asks first. The plus makes a new note
// and opens it. The phone reads and writes through a connected device
// that has the project, from `remoteNotesReach`.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useMemo } from "react";
import { Alert, FlatList, Pressable, StyleSheet, Text, View } from "react-native";

import type { NoteId, ProjectId } from "@maxhq/shared/bindings";
import { useNow } from "@maxhq/shared/lib/clock";
import { changeHead, deleteNoteQuestion, makeNote } from "@maxhq/shared/notes/actions";
import { formatAgoShort } from "@maxhq/shared/status/format";
import {
	type NoteRow,
	notesOf,
	remoteNotesReach,
	savedBy,
	shownTitle,
} from "@maxhq/shared/view/notes";

import type { ScreenProps } from "../../navigation";
import { type Services, useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { choose, confirm } from "../../ui/choose";
import { Header, IconButton } from "../../ui/Header";
import { Plus } from "../../ui/Icon";

/** Makes a note in the project and opens it, or says why it couldn't. */
export const openNewNote = async (
	services: Services,
	project: ProjectId,
	open: (note: NoteId) => void
): Promise<void> => {
	const { view } = services.view.getState();
	const made = await makeNote(services.source, view, remoteNotesReach(view, project), project);
	if ("failure" in made) Alert.alert(t("Couldn't make the note"), made.failure);
	else open(made.note);
};

const askToDelete = (services: Services, project: ProjectId, row: NoteRow) =>
	confirm(deleteNoteQuestion(shownTitle(row.title)), t("Delete"), () => {
		const reach = remoteNotesReach(services.view.getState().view, project);
		void changeHead(services.source, reach, project, row.note, { deleted: true }).then(
			(failure) => {
				if (failure !== null) Alert.alert(t("Couldn't delete the note"), failure);
			}
		);
	});

export const NotesScreen = ({ navigation, route }: ScreenProps<"Notes">) => {
	const { t } = useI18n();

	const { project } = route.params;
	const services = useServices();
	const view = useView((s) => s.view);
	const now = useNow(60_000);
	const rows = useMemo(() => notesOf(view, project), [view, project]);
	const reach = remoteNotesReach(view, project);
	const name = view.tables.project.get(project)?.name;
	const open = (note: NoteId, made?: boolean) =>
		navigation.navigate("Note", { project, note, made });

	return (
		<View style={styles.screen}>
			<Header
				title={t("Notes")}
				subtitle={name ? <Text style={styles.sub}>{name}</Text> : undefined}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				right={
					reach.readOnly ? null : (
						<IconButton
							label={t("New note")}
							onPress={() =>
								void openNewNote(services, project, (note) => open(note, true))
							}
						>
							<Plus size={16} />
						</IconButton>
					)
				}
			/>
			<FlatList
				data={rows}
				keyExtractor={(row) => row.note}
				contentContainerStyle={styles.content}
				renderItem={({ item }) => (
					<Pressable
						accessibilityRole="link"
						onPress={() => open(item.note)}
						onLongPress={
							reach.readOnly
								? undefined
								: () =>
										choose(shownTitle(item.shown), [
											{
												label: t("Delete"),
												destructive: true,
												pick: () => askToDelete(services, project, item),
											},
										])
						}
						style={({ pressed }) => [styles.row, pressed && styles.pressed]}
					>
						<View style={styles.text}>
							<Text style={styles.title} numberOfLines={1}>
								{shownTitle(item.shown)}
							</Text>
							<Text style={styles.meta} numberOfLines={1}>
								{savedBy(view, item.by)}
							</Text>
						</View>
						<Text style={styles.meta}>{formatAgoShort(item.at, now)}</Text>
					</Pressable>
				)}
				ListEmptyComponent={
					<Text style={styles.note}>
						{reach.readOnly
							? t("No notes here. {{reason}}", { reason: reach.why ?? "" }).trim()
							: t("No notes yet. Tap the plus to write one.")}
					</Text>
				}
			/>
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	content: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 32 },
	sub: { color: color.mutedForeground, fontSize: size.meta },
	row: {
		minHeight: 52,
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingHorizontal: 10,
		borderRadius: 6,
	},
	pressed: { backgroundColor: color.accent },
	text: { flex: 1, minWidth: 0, gap: 1 },
	title: { color: color.foregroundStrong, fontSize: size.body },
	meta: { color: color.mutedForeground, fontSize: size.meta },
	note: { color: color.mutedForeground, fontSize: size.small, padding: 16 },
});
