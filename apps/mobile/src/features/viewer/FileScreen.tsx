// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A file on an agent's device, read-only, as the desktop's file pane shows
// it: its name and path at the top, then the file. It comes through the
// media cache, up to the viewer's limit for its kind, and a larger one waits
// for Show anyway. What it draws as is in file.ts. More holds Copy path, and
// Share on iOS, where Save to Files is, or Save on Android, which puts it in
// a folder you pick, as a visual's Save as .html does. A file the viewer
// hasn't fetched whole, such as one too large to show, downloads first.
// How far it has got shows in the note in the file's place, or under the
// file's name once the file itself shows, as after Show anyway, and Cancel
// is in the note or in More.

import { t, useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { File, FileMode } from "expo-file-system";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Image, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { WebView } from "react-native-webview";

import { BINARY_SNIFF, looksBinary, textLines } from "@maxhq/shared/agent-view/code-text";
import {
	cancelDownload,
	type DownloadState,
	downloadOf,
	startDownload,
	useDownload,
} from "@maxhq/shared/agent-view/downloads";
import { FULL_LIMIT, loadMedia, mediaLost, useMedia } from "@maxhq/shared/agent-view/media";
import {
	fileName,
	folderOf,
	showAnywayBlocked,
	viewerLimit,
} from "@maxhq/shared/agent-view/viewer-files";
import type { AgentId } from "@maxhq/shared/bindings";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";

import { urlOf } from "../../data/native-source";
import type { ScreenProps } from "../../navigation";
import { OS, platform } from "../../platform";
import { saveAs } from "../../platform/save";
import { useServices, useView } from "../../services";
import { color, size } from "../../theme";
import { Button } from "../../ui/Button";
import { choose } from "../../ui/choose";
import { Spinner } from "../../ui/Dot";
import { Header, IconButton } from "../../ui/Header";
import { More } from "../../ui/Icon";
import { MarkdownText } from "../../ui/MarkdownText";
import { CodeLines } from "./CodeLines";
import { binaryNote, fileNote, headerSaveText, lookOf, mimeOf, noViewNote, saveText } from "./file";

// The words of an error something threw.
const said = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** What Save or Share is called here. */
const saveLabel = (): string => (platform.savesByShare ? t("Share") : t("Save"));

/**
 * Hands a copy of the file on, through the share sheet on iOS and into a
 * picked folder on Android. A download is `owned`, so it moves rather than
 * copies, and none of it stays behind.
 */
const saveFile = async (url: string, path: string, owned = false): Promise<void> => {
	const name = fileName(path);
	try {
		if (await saveAs(name, mimeOf(path), { file: new File(url), owned })) {
			Alert.alert(t("Saved"), name);
		}
	} catch (error) {
		Alert.alert(t("Couldn't save {{what}}", { what: name }), said(error));
	}
};

/**
 * Save for the file at `path`: from the viewer's copy at `url` once it has
 * the whole file, and through a download otherwise. A download that ends
 * after the screen closed waits for the next Save.
 */
const useSave = (
	agent: AgentId,
	path: string,
	url: string | null
): {
	readonly download: DownloadState | null;
	readonly save: () => Promise<void>;
	readonly cancel: () => void;
} => {
	const { source } = useServices();
	const download = useDownload(agent, path);
	const live = useRef(true);
	useEffect(() => {
		live.current = true;
		return () => {
			live.current = false;
		};
	}, []);
	const save = async () => {
		if (url) return saveFile(url, path);
		const before = downloadOf(agent, path);
		if (before?.state === "downloading") return;
		if (before?.state !== "done" || !new File(urlOf(before.path)).exists)
			await startDownload(source, agent, path);
		const landed = downloadOf(agent, path);
		if (!live.current || landed === null || landed.state === "downloading") return;
		// Forgotten either way, since the file moves on and a failure says why now.
		cancelDownload(source, agent, path);
		if (landed.state === "failed") {
			Alert.alert(t("Couldn't save {{what}}", { what: fileName(path) }), landed.reason);
			return;
		}
		await saveFile(urlOf(landed.path), path, true);
	};
	return { download, save, cancel: () => cancelDownload(source, agent, path) };
};

type Save = ReturnType<typeof useSave>;

/** Save for a file the viewer won't draw, or Cancel while its download is on its way. */
const SaveAction = ({ save }: { save: Save }) => {
	const { t } = useI18n();
	if (save.download?.state === "downloading")
		return <Button small label={t("Cancel")} onPress={save.cancel} />;
	return <Button small label={saveLabel()} onPress={() => void save.save()} />;
};

/**
 * Reads a file in the cache as text, or gives null when it's binary: when a
 * zero byte comes in its first 8 KB, as the desktop tells.
 */
const readText = async (url: string, size: number): Promise<string | null> => {
	const file = new File(url);
	if (size > 0) {
		const handle = file.open(FileMode.ReadOnly);
		try {
			if (looksBinary(await handle.readBytes(Math.min(size, BINARY_SNIFF)))) return null;
		} finally {
			handle.close();
		}
	}
	return await file.text();
};

type Read =
	| { readonly url: string; readonly text: string | null }
	| { readonly url: string; readonly failure: string };

/** A file's text once it's read, null while it's read, and a way to read it again. */
const useText = (url: string, size: number): [Read | null, () => void] => {
	const [read, setRead] = useState<Read | null>(null);
	const [reads, setReads] = useState(0);
	useEffect(() => {
		let live = true;
		readText(url, size).then(
			(text) => live && setRead({ url, text }),
			(error: unknown) => live && setRead({ url, failure: said(error) })
		);
		return () => {
			live = false;
		};
	}, [url, size, reads]);
	return [read && read.url === url ? read : null, () => setReads((n) => n + 1)];
};

/** What shows in the file's place: why, with what you can do about it, and how far Save has got. */
const Note = ({
	text,
	busy = false,
	saving = null,
	children,
}: {
	text: string;
	busy?: boolean;
	saving?: string | null;
	children?: ReactNode;
}) => (
	<View
		accessibilityRole={busy ? "progressbar" : "alert"}
		accessibilityLiveRegion="polite"
		style={styles.note}
	>
		{busy ? <Spinner /> : null}
		<Text style={styles.noteText}>{text}</Text>
		{saving ? <Text style={styles.saving}>{saving}</Text> : null}
		{children ? <View style={styles.actions}>{children}</View> : null}
	</View>
);

// A file drawn as numbered lines, once its text is read.
const TextFile = ({
	path,
	url,
	size,
	line,
	ask,
}: {
	path: string;
	url: string;
	size: number;
	line: number | undefined;
	ask: object;
}) => {
	const { t } = useI18n();
	const [read, again] = useText(url, size);
	const text = read && "text" in read ? read.text : null;
	const lines = useMemo(() => (text === null ? null : textLines(text)), [text]);
	if (!read) return <Note busy text={t("Reading…")} />;
	if ("failure" in read)
		return (
			<Note
				text={t("Couldn't read {{name}}. {{reason}}", {
					name: fileName(path),
					reason: read.failure,
				})}
			>
				<Button small label={t("Try again")} onPress={again} />
			</Note>
		);
	if (!lines) return <Note text={binaryNote(path)} />;
	return <CodeLines path={path} lines={lines} bytes={size} line={line} ask={ask} />;
};

// A Markdown file, drawn as a reply is.
const MarkdownFile = ({
	agent,
	path,
	url,
	size,
}: {
	agent: AgentId;
	path: string;
	url: string;
	size: number;
}) => {
	const { t } = useI18n();
	const insets = useSafeAreaInsets();
	const [read, again] = useText(url, size);
	if (!read) return <Note busy text={t("Reading…")} />;
	if ("failure" in read)
		return (
			<Note
				text={t("Couldn't read {{name}}. {{reason}}", {
					name: fileName(path),
					reason: read.failure,
				})}
			>
				<Button small label={t("Try again")} onPress={again} />
			</Note>
		);
	if (read.text === null) return <Note text={binaryNote(path)} />;
	return (
		<ScrollView
			style={styles.fill}
			contentContainerStyle={[styles.page, { paddingBottom: insets.bottom + 24 }]}
		>
			<MarkdownText text={read.text} agent={agent} folder={folderOf(path)} />
		</ScrollView>
	);
};

// A picture, filling the space under the header, where a pinch zooms it on iOS.
const ImageFile = ({ agent, path, url }: { agent: AgentId; path: string; url: string }) => {
	const { t } = useI18n();
	const { source } = useServices();
	const [box, setBox] = useState<{ width: number; height: number } | null>(null);
	const name = fileName(path);
	return (
		<View
			style={styles.fill}
			onLayout={(event) => {
				const { width, height } = event.nativeEvent.layout;
				setBox({ width, height });
			}}
		>
			{box ? (
				<ScrollView
					style={styles.fill}
					contentContainerStyle={styles.center}
					maximumZoomScale={5}
					minimumZoomScale={1}
					centerContent
					showsHorizontalScrollIndicator={false}
					showsVerticalScrollIndicator={false}
				>
					<Image
						accessible
						accessibilityRole="image"
						accessibilityLabel={name}
						source={{ uri: url }}
						resizeMode="contain"
						onError={() =>
							void mediaLost(
								source,
								agent,
								path,
								t("Couldn't show {{name}}.", { name })
							)
						}
						style={box}
					/>
				</ScrollView>
			) : null}
		</View>
	);
};

// The page loads only the file itself and runs no script.
const stays = ({ url }: { url: string }) => url.startsWith("file:") || url.startsWith("about:");

// A PDF, in a web view, which iOS draws.
const PdfFile = ({ path, url }: { path: string; url: string }) => {
	const { t } = useI18n();
	const [failed, setFailed] = useState(false);
	if (failed) return <Note text={t("Couldn't show {{name}}.", { name: fileName(path) })} />;
	return (
		<WebView
			source={{ uri: url }}
			style={styles.web}
			originWhitelist={["*"]}
			onShouldStartLoadWithRequest={stays}
			javaScriptEnabled={false}
			setSupportMultipleWindows={false}
			allowsLinkPreview={false}
			dataDetectorTypes="none"
			onError={() => setFailed(true)}
		/>
	);
};

/** What the screen shows below its header, as the file's fetch stands. */
const Body = ({
	agent,
	path,
	line,
	ask,
	save,
}: {
	agent: AgentId;
	path: string;
	line: number | undefined;
	ask: object;
	save: Save;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const media = useMedia(agent, path);
	const saving = saveText(save.download);
	// Show anyway asks for the whole file, as far as the app's cache takes.
	const [limit, setLimit] = useState(() => viewerLimit(path));
	useEffect(() => {
		void loadMedia(source, agent, path, limit);
	}, [source, agent, path, limit]);
	const blocked = useView((s) => showAnywayBlocked(s.view, agent));
	const note = fileNote(path, media, blocked);

	if (note || media?.state !== "ready") {
		switch (note?.kind) {
			case "large":
				return (
					<Note text={note.text} saving={saving}>
						{note.more ? (
							<Button
								small
								label={t("Show anyway")}
								onPress={() => setLimit(FULL_LIMIT)}
							/>
						) : null}
						<SaveAction save={save} />
					</Note>
				);
			case "failed":
				return (
					<Note text={note.text} saving={saving}>
						<Button
							small
							label={t("Try again")}
							onPress={() => void loadMedia(source, agent, path, limit)}
						/>
					</Note>
				);
			default:
				return <Note busy text={note?.text ?? t("Reading…")} saving={saving} />;
		}
	}

	const { url, size } = media;
	switch (lookOf(path, OS)) {
		case "markdown":
			return <MarkdownFile key={url} agent={agent} path={path} url={url} size={size} />;
		case "text":
			return <TextFile key={url} path={path} url={url} size={size} line={line} ask={ask} />;
		case "image":
			return <ImageFile key={url} agent={agent} path={path} url={url} />;
		case "pdf":
			return <PdfFile key={url} path={path} url={url} />;
		case "none":
			return (
				<Note text={noViewNote(path, size, OS)} saving={saving}>
					<SaveAction save={save} />
				</Note>
			);
	}
};

/** A file on an agent's device, read-only, at a line when one's asked for. */
export const FileScreen = ({ navigation, route }: ScreenProps<"File">) => {
	const { t } = useI18n();
	const { params } = route;
	const { agent, path } = params;
	const name = fileName(path);
	const media = useMedia(agent, path);
	const url = media?.state === "ready" ? media.url : null;
	const save = useSave(agent, path, url);
	const [copied, copy] = useCopyWith(Clipboard.setStringAsync);
	const more = () =>
		choose(name, [
			{ label: t("Copy path"), pick: () => copy(path) },
			save.download?.state === "downloading"
				? { label: t("Cancel downloading {{name}}", { name }), pick: save.cancel }
				: { label: saveLabel(), pick: () => void save.save() },
		]);
	return (
		<View style={styles.screen}>
			<Header
				title={name}
				subtitle={
					<Text
						accessibilityLiveRegion="polite"
						numberOfLines={1}
						ellipsizeMode="head"
						style={styles.path}
					>
						{copyLabel(copied, headerSaveText(path, media, save.download, OS) ?? path)}
					</Text>
				}
				back={{ label: t("Back"), onPress: () => navigation.goBack() }}
				right={
					<IconButton label={t("More")} onPress={more}>
						<More />
					</IconButton>
				}
			/>
			<Body agent={agent} path={path} line={params.line} ask={params} save={save} />
		</View>
	);
};

const styles = StyleSheet.create({
	screen: { flex: 1, backgroundColor: color.background },
	fill: { flex: 1 },
	center: { flexGrow: 1, alignItems: "center", justifyContent: "center" },
	path: { color: color.mutedForeground, fontSize: size.meta },
	page: { paddingHorizontal: 16, paddingTop: 12 },
	web: { flex: 1, backgroundColor: color.background },
	note: {
		flex: 1,
		alignItems: "center",
		justifyContent: "center",
		gap: 12,
		paddingHorizontal: 32,
	},
	noteText: {
		color: color.mutedForeground,
		fontSize: size.small,
		textAlign: "center",
	},
	saving: { color: color.mutedForeground, fontSize: size.meta, fontVariant: ["tabular-nums"] },
	actions: { flexDirection: "row", gap: 8 },
});
