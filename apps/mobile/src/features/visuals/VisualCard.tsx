// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A visual in a feed or a note, drawn where it is: its title and Open, which
// shows it whole, and More, with what the phone can do with it, over its
// page. The page runs once the card has its place. It draws as tall as it
// is, up to most of the screen, and scrolls inside past that. Save as .html
// goes through the share sheet on iOS, where Save to Files is, and into a
// folder you pick on Android, whose share sheet takes no files. Add to note
// is there in an agent's feed, for the notes of the agent's project. Tell
// the agent, on the error bar, quotes the page's errors into the agent's
// box, only for a card in that agent's own feed while the box shows.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useNavigation } from "@react-navigation/native";
import type { NativeStackNavigationProp } from "@react-navigation/native-stack";
import * as Clipboard from "expo-clipboard";
import { Directory, File, Paths } from "expo-file-system";
import { memo, useMemo, useState } from "react";
import {
	Alert,
	Platform,
	Pressable,
	Share,
	StyleSheet,
	Text,
	useWindowDimensions,
	View,
} from "react-native";

import type { AgentId, NoteId, ProjectId } from "@maxhq/shared/bindings";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";
import {
	addToHead,
	addToNewNote,
	addToNoteTarget,
	takeOutOfHead,
} from "@maxhq/shared/notes/add-visual";
import type { Placed } from "@maxhq/shared/notes/visuals";
import { quoteInbox } from "@maxhq/shared/pills/quote-inbox";
import { notesOf, remoteNotesReach, untitled } from "@maxhq/shared/view/notes";
import type { Visual } from "@maxhq/shared/visuals/fence";
import { tellQuote, useVisualOrigin } from "@maxhq/shared/visuals/origin";
import { visualFileName } from "@maxhq/shared/visuals/visual";

import type { Screens } from "../../navigation";
import { type Services, useServices } from "../../services";
import { color, radius, size, TOUCH } from "../../theme";
import { choose } from "../../ui/choose";
import { More } from "../../ui/Icon";
import { useFeed } from "../agent/feed-context";
import { cardCap, cardHeight, heightKey, knownHeight, sameVisual } from "./card";
import { openVisual, type VisualToOpen, visualName } from "./visual";
import { NoPage, type NoPageWhy, type Tell, useDeviceHtml, VisualPage } from "./VisualPage";

const HEADER = 32;

// The words of an error something threw.
const said = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** Saves a visual's page as a .html file, or says why it couldn't. */
const saveHtml = async ({ html, title }: Visual): Promise<void> => {
	const name = visualFileName(title);
	try {
		if (Platform.OS === "ios") {
			const file = new File(Paths.cache, name);
			await file.write(html);
			await Share.share({ url: file.uri });
			return;
		}
		const folder = await Directory.pickDirectoryAsync().catch(() => null);
		// Closed without picking one.
		if (!folder) return;
		await folder.createFile(name, "text/html").write(html);
		Alert.alert(t("Saved"), name);
	} catch (error) {
		Alert.alert(t("Couldn't save {{what}}", { what: name }), said(error));
	}
};

type Go = (project: ProjectId, note: NoteId, made: boolean) => void;

/** Says a visual went into a note, with Undo, which takes out exactly that block, and Open. */
const showAdded = (
	services: Services,
	{
		project,
		note,
		title,
		made,
	}: { project: ProjectId; note: NoteId; title: string; made: boolean },
	placed: Placed,
	go: Go
) => {
	const undo = async () => {
		const reach = remoteNotesReach(services.view.getState().view, project);
		const why = await takeOutOfHead(services.source, reach, project, note, placed);
		if (why) Alert.alert(why);
	};
	Alert.alert(t("Added to"), title, [
		{ text: t("Undo"), style: "destructive", onPress: () => void undo() },
		{ text: t("Open"), onPress: () => go(project, note, made) },
		{ text: t("Done"), style: "cancel" },
	]);
};

/** Asks which of the project's notes takes the visual, or a new one, and puts it there. */
const addToNote = (services: Services, project: ProjectId, visual: Visual, go: Go) => {
	const { source } = services;
	const view = services.view.getState().view;
	const reach = remoteNotesReach(view, project);
	const into = async (note: NoteId, title: string) => {
		const added = await addToHead(source, reach, project, note, visual);
		if ("failure" in added) Alert.alert(t("Add to note"), added.failure);
		else showAdded(services, { project, note, title, made: false }, added.placed, go);
	};
	const intoNew = async () => {
		const made = await addToNewNote(source, view, reach, project, visual);
		if ("failure" in made) Alert.alert(t("Add to note"), made.failure);
		else {
			// The view may not list it yet, and it's named as it was made.
			const title = untitled(view, project);
			showAdded(services, { project, note: made.note, title, made: true }, made.placed, go);
		}
	};
	choose(t("Add to note"), [
		...notesOf(view, project).map((row) => ({
			label: row.shown,
			pick: () => void into(row.note, row.shown),
		})),
		{ label: t("New note"), pick: () => void intoNew() },
	]);
};

// The agent whose feed the card is in, or null in a note, which has no feed.
const useFeedAgent = (): AgentId | null => {
	try {
		return useFeed().agent;
	} catch {
		return null;
	}
};

const Card = ({
	visual,
	meta,
	html,
	why,
}: {
	visual: VisualToOpen;
	meta: string | null | undefined;
	/** The page, once it's here. */
	html: string | null;
	/** Why the page didn't come, when it didn't. */
	why: NoPageWhy | null;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const navigation = useNavigation<NativeStackNavigationProp<Screens>>();
	const agent = useFeedAgent() ?? (visual.kind === "device" ? visual.agent : null);
	const { height: screen } = useWindowDimensions();
	const [laid, setLaid] = useState(false);
	const [runs, setRuns] = useState(0);
	const [copied, copy] = useCopyWith(Clipboard.setStringAsync);
	const mark = useMemo(() => heightKey(visual), [visual]);
	const name = visualName(visual.title);
	const cap = cardCap(screen);
	const card = useMemo(() => ({ cap, mark }), [cap, mark]);

	const origin = useVisualOrigin();
	const onTell = useMemo<Tell | undefined>(
		() =>
			origin && html !== null
				? (errors, count) =>
						quoteInbox.send(
							origin.agent.id,
							tellQuote(origin, html, visual.title, errors, count),
							true
						)
				: undefined,
		[origin, html, visual.title]
	);

	const go: Go = (project, note, made) => navigation.navigate("Note", { project, note, made });
	// The sheet reads the view as it is when you tap, so Add to note offers the notes there now.
	const more = () => {
		const page = html === null ? null : { html, title: visual.title };
		const target =
			page && agent !== null
				? addToNoteTarget(
						services.view.getState().view,
						{ agent, project: null },
						remoteNotesReach
					)
				: null;
		choose(name, [
			{ label: t("Copy source"), disabled: !page, pick: () => page && copy(page.html) },
			...(page && target
				? [
						{
							label: t("Add to note"),
							pick: () => addToNote(services, target.project, page, go),
						},
					]
				: []),
			{
				label: t("Save as .html…"),
				disabled: !page,
				pick: () => page && void saveHtml(page),
			},
			{ label: t("Reload"), disabled: !page, pick: () => setRuns((n) => n + 1) },
		]);
	};

	const height = cardHeight(knownHeight(mark), cap).height;
	return (
		<View style={styles.card} onLayout={laid ? undefined : () => setLaid(true)}>
			<View style={styles.header}>
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={t("Open the visual {{name}}", { name })}
					onPress={() => openVisual(visual)}
					style={({ pressed }) => [styles.open, pressed ? styles.pressed : null]}
				>
					<Text style={styles.diamond}>◆</Text>
					<Text numberOfLines={1} style={styles.title}>
						{name}
					</Text>
					{meta ? <Text style={styles.meta}>{meta}</Text> : null}
					<Text style={styles.dot}>·</Text>
					<Text style={styles.openText}>{t("Open")}</Text>
				</Pressable>
				<View style={styles.grow} />
				{copied === "idle" ? null : (
					<Text accessibilityLiveRegion="polite" style={styles.meta}>
						{copyLabel(copied, "")}
					</Text>
				)}
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={t("More")}
					onPress={more}
					hitSlop={(TOUCH - HEADER) / 2}
					style={({ pressed }) => [styles.more, pressed ? styles.pressed : null]}
				>
					<More size={16} />
				</Pressable>
			</View>
			{html === null ? (
				<NoPage why={why} height={height} />
			) : laid ? (
				<VisualPage
					html={html}
					card={card}
					runs={runs}
					from={origin?.agent.title ?? null}
					onTell={onTell}
				/>
			) : (
				<View style={{ height }} />
			)}
		</View>
	);
};

// A visual on an agent's device, whose page comes through the media cache.
const DeviceCard = ({
	visual,
	meta,
}: {
	visual: VisualToOpen & { readonly kind: "device" };
	meta: string | null | undefined;
}) => {
	const page = useDeviceHtml(visual.agent, visual.path);
	return (
		<Card
			visual={visual}
			meta={meta}
			html={page && "html" in page ? page.html : null}
			why={page && "failure" in page ? page : null}
		/>
	);
};

const AnyCard = ({ visual, meta }: { visual: VisualToOpen; meta?: string | null }) =>
	visual.kind === "page" ? (
		<Card visual={visual} meta={meta} html={visual.html} why={null} />
	) : (
		<DeviceCard key={`${visual.agent}\n${visual.path}`} visual={visual} meta={meta} />
	);

/**
 * A visual's card, which draws its page where it is. A reply that streams
 * on draws it again with a new but equal visual, which keeps the card.
 */
export const VisualCard = memo(
	AnyCard,
	(a, b) => sameVisual(a.visual, b.visual) && a.meta === b.meta
);

const styles = StyleSheet.create({
	card: {
		alignSelf: "stretch",
		overflow: "hidden",
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.background,
	},
	header: {
		height: HEADER,
		flexDirection: "row",
		alignItems: "center",
		paddingLeft: 4,
		paddingRight: 2,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	open: {
		flexShrink: 1,
		height: HEADER,
		flexDirection: "row",
		alignItems: "center",
		gap: 6,
		paddingHorizontal: 6,
		borderRadius: radius.sm,
	},
	pressed: { backgroundColor: color.accent },
	diamond: { color: color.visual, fontSize: size.small },
	title: { flexShrink: 1, color: color.foreground, fontSize: size.small, fontWeight: "600" },
	meta: { flexShrink: 0, color: color.mutedForeground, fontSize: size.meta },
	dot: { color: color.mutedForeground, fontSize: size.small },
	openText: { color: color.visual, fontSize: size.small },
	grow: { flex: 1 },
	more: {
		width: HEADER,
		height: HEADER,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: radius.sm,
	},
});
