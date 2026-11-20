// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Pictures in an agent's feed, in the same states as the desktop's: one a
// reply names, in the column, and one you sent, an agent sent or a tool gave
// back, as a small frame with its name. A tap on either shows it whole, where
// a pinch zooms it. One on the agent's device comes through the media cache,
// the same way for an agent here, on another device or in a container. One up
// to the on-sight limit comes once its row mounts, which is when the list is
// about to show it, and a larger one waits for a tap. A block of a reply
// shows its first pictures and the rest wait for a tap, and a file's preview
// shows only in the form maxd makes one, as the shared pictures module says.
// One iOS dropped from the cache while the app was away comes again once on
// its own before it says it couldn't show.
//
// The viewer sits over the whole app, so a row that scrolls out of the list's
// window while the agent works doesn't close it.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { useEffect, useState } from "react";
import {
	Image,
	Modal,
	Pressable,
	ScrollView,
	StyleSheet,
	Text,
	useWindowDimensions,
	View,
	type ViewStyle,
} from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import type { AgentId, AttachmentId, AttachmentRef } from "@maxhq/shared/bindings";
import { formatBytes } from "@maxhq/shared/agent-view/format";
import {
	cancelMedia,
	FULL_LIMIT,
	loadMedia,
	mediaLost,
	PICTURE_LIMIT,
	useMedia,
} from "@maxhq/shared/agent-view/media";
import { morePictures, previewOf, shownPictures } from "@maxhq/shared/agent-view/pictures";

import { useServices } from "../services";
import { color, radius, size, tint } from "../theme";
import { Button } from "./Button";
import { Spinner } from "./Dot";
import { Close } from "./Icon";
import type { BlockPicture } from "./markdown";

/** A picture to show whole: one the app can load now, or one on the agent's device. */
type Whole = { readonly name: string } & (
	| { readonly kind: "url"; readonly url: string }
	| {
			readonly kind: "device";
			readonly agent: AgentId;
			readonly path: string;
			/** The small preview to show while it comes. */
			readonly thumbnail: string | null;
	  }
);

const shown = createStore<{ whole: Whole | null }>(() => ({ whole: null }));

const showWhole = (whole: Whole) => shown.setState({ whole });

const closeWhole = () => shown.setState({ whole: null });

/** Shows a picture an agent has whole, as a tap on its thumbnail does. */
export const showPicture = (agent: AgentId, file: AttachmentRef): void =>
	showWhole({
		kind: "device",
		agent,
		path: file.path,
		name: file.name,
		thumbnail: previewOf(file),
	});

const percent = (sent: number, total: number) =>
	total > 0 ? Math.min(100, Math.floor((sent / total) * 100)) : null;

/** A picture's width and height. */
type Size = { readonly width: number; readonly height: number };

// The most a picture in a reply takes either way.
const MOST = 480;

// What pictures measure, so a row that scrolls back lays out at once.
const MEASURED_MOST = 256;
const measured = new Map<string, Size>();

const remember = (url: string, natural: Size) => {
	if (measured.size >= MEASURED_MOST) measured.clear();
	measured.set(url, natural);
};

/** How large a picture is, or null until it's known. Key it by `url`. */
const useNaturalSize = (url: string, onFail: () => void): Size | null => {
	const [natural, setNatural] = useState(() => measured.get(url) ?? null);
	const known = natural !== null;
	useEffect(() => {
		if (known) return;
		let live = true;
		Image.getSize(url).then(
			({ width, height }) => {
				if (width <= 0 || height <= 0) {
					if (live) onFail();
					return;
				}
				remember(url, { width, height });
				if (live) setNatural({ width, height });
			},
			() => {
				if (live) onFail();
			}
		);
		return () => {
			live = false;
		};
		// A row's `onFail` says the same thing on each render.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [url, known]);
	return natural;
};

// A picture's frame: the column's width at most, never past its own size.
const frameOf = ({ width, height }: Size): ViewStyle => {
	const ratio = width / height;
	return { width: "100%", maxWidth: Math.min(width, MOST, MOST * ratio), aspectRatio: ratio };
};

/** A reply's picture the app can load, which shows whole on a tap. */
const ShownPicture = ({
	url,
	label,
	whole,
	onFail,
}: {
	url: string;
	label: string;
	whole: Whole;
	onFail: () => void;
}) => {
	const { t } = useI18n();

	const natural = useNaturalSize(url, onFail);
	return natural ? (
		<Pressable
			accessibilityRole="imagebutton"
			accessibilityLabel={t("Show {{name}} full size", { name: label })}
			onPress={() => showWhole(whole)}
			style={[styles.frame, frameOf(natural)]}
		>
			<Image
				source={{ uri: url }}
				resizeMode="contain"
				onError={onFail}
				style={StyleSheet.absoluteFill}
			/>
		</Pressable>
	) : (
		<View style={[styles.frame, styles.waiting]} />
	);
};

/** Why a reply's picture doesn't show, with a way to try again. */
const Unavailable = ({
	label,
	detail,
	onRetry,
}: {
	label: string;
	detail?: string;
	onRetry?: () => void;
}) => {
	const { t } = useI18n();
	return (
		<View accessibilityRole="alert" style={styles.note}>
			<Text style={styles.noteText}>
				{[t("Image unavailable"), label, detail].filter(Boolean).join(" · ")}
			</Text>
			{onRetry ? <Button small label={t("Try again")} onPress={onRetry} /> : null}
		</View>
	);
};

// A picture in the text itself.
const InlinePicture = ({ uri, label }: { uri: string; label: string }) => {
	const [failed, setFailed] = useState(false);
	return failed ? (
		<Unavailable label={label} />
	) : (
		<ShownPicture
			url={uri}
			label={label}
			whole={{ kind: "url", url: uri, name: label }}
			onFail={() => setFailed(true)}
		/>
	);
};

// A picture on the agent's device.
const DevicePicture = ({
	agent,
	path,
	name,
	label,
}: {
	agent: AgentId;
	path: string;
	name: string;
	label: string;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const media = useMedia(agent, path);
	useEffect(() => {
		void loadMedia(source, agent, path, PICTURE_LIMIT);
	}, [source, agent, path]);
	const loadAll = () => void loadMedia(source, agent, path, FULL_LIMIT);

	switch (media?.state) {
		case "ready":
			return (
				<ShownPicture
					key={media.url}
					url={media.url}
					label={label}
					whole={{ kind: "url", url: media.url, name: label }}
					onFail={() =>
						void mediaLost(source, agent, path, t("Couldn't show {{name}}.", { name }))
					}
				/>
			);
		case "failed":
			return <Unavailable label={label} detail={media.reason} onRetry={loadAll} />;
		case "large":
			return media.size <= FULL_LIMIT ? (
				<View style={styles.note}>
					<Button
						small
						label={t("Show picture, {{size}}", { size: formatBytes(media.size) })}
						onPress={loadAll}
					/>
				</View>
			) : (
				<View style={styles.note}>
					<Text style={styles.noteText}>
						{t("Picture too large to show here, {{size}}", {
							size: formatBytes(media.size),
						})}
					</Text>
				</View>
			);
		default: {
			const done = media?.state === "loading" ? percent(media.sent, media.total) : null;
			return (
				<View
					accessible
					accessibilityRole="progressbar"
					accessibilityLabel={t("Loading {{name}}", { name: label })}
					style={[styles.frame, styles.waiting]}
				>
					<Spinner />
					{done !== null ? (
						<Text style={styles.percent}>
							{formatNumber(done / 100, { style: "percent" })}
						</Text>
					) : null}
				</View>
			);
		}
	}
};

/** A picture a reply names, in the column under the text around it. */
export const ReplyPicture = ({ agent, picture }: { agent: AgentId; picture: BlockPicture }) => {
	const { source, alt } = picture;
	const label = alt || source.name;
	return source.access === "inline" ? (
		<InlinePicture key={source.uri} uri={source.uri} label={label} />
	) : (
		<DevicePicture agent={agent} path={source.path} name={source.name} label={label} />
	);
};

// A file as a small frame with its name. One with no preview, such as one read
// back from history, comes from the device to fill the frame.
const PictureThumb = ({
	agent,
	file,
	number,
}: {
	agent: AgentId;
	file: AttachmentRef;
	/** The number its pill shows, in a corner. */
	number?: number;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const media = useMedia(agent, file.path);
	const { path, name } = file;
	const thumbnail = previewOf(file);
	useEffect(() => {
		if (!thumbnail) void loadMedia(source, agent, path, PICTURE_LIMIT);
	}, [source, agent, path, thumbnail]);
	const face = thumbnail ?? (media?.state === "ready" ? media.url : null);
	return (
		<Pressable
			accessibilityRole="imagebutton"
			accessibilityLabel={t("Show {{name}} full size", { name })}
			onPress={() => showWhole({ kind: "device", agent, path, name, thumbnail })}
			style={styles.thumb}
		>
			{face ? (
				<Image source={{ uri: face }} resizeMode="cover" style={StyleSheet.absoluteFill} />
			) : media?.state === "loading" ? (
				<Spinner />
			) : media?.state === "failed" ? (
				<Text style={styles.thumbNote}>{t("Couldn't load. Tap to try again.")}</Text>
			) : media?.state === "large" ? (
				<Text style={styles.thumbNote}>{t("Too large to preview")}</Text>
			) : null}
			{number !== undefined ? (
				<View style={styles.number}>
					<Text style={styles.numberText}>
						{t("Image {{number}}", { number: formatNumber(number) })}
					</Text>
				</View>
			) : null}
			<View style={styles.strip}>
				<Text numberOfLines={1} style={styles.stripText}>
					{name}
				</Text>
			</View>
		</Pressable>
	);
};

/** The pictures a block of a reply names, the first few until you ask for the rest. */
export const ReplyPictures = ({
	agent,
	pictures,
}: {
	agent: AgentId;
	pictures: readonly BlockPicture[];
}) => {
	const [all, setAll] = useState(false);
	const { shown, more } = shownPictures(pictures, all);
	return (
		<>
			{shown.map((picture, i) => (
				// A picture's place is its identity, as a block's is.
				<ReplyPicture key={i} agent={agent} picture={picture} />
			))}
			{more > 0 ? (
				<View style={styles.more}>
					<Button small label={morePictures(more)} onPress={() => setAll(true)} />
				</View>
			) : null}
		</>
	);
};

/** The pictures a row carries, in a row that wraps. */
export const PictureThumbs = ({
	agent,
	files,
	numbers,
	style,
}: {
	agent: AgentId;
	files: readonly AttachmentRef[];
	/** The number each placed picture shows on its pill. */
	numbers?: ReadonlyMap<AttachmentId, number>;
	style?: ViewStyle;
}) => {
	return files.length > 0 ? (
		<View style={[styles.thumbs, style]}>
			{files.map((file) => (
				<PictureThumb
					key={file.id}
					agent={agent}
					file={file}
					number={numbers?.get(file.id)}
				/>
			))}
		</View>
	) : null;
};

// A picture filling the screen, where a pinch zooms it. It decodes at the
// screen's size, so a very large one softens at the far end of the zoom.
const Zoomed = ({ url, label, onFail }: { url: string; label: string; onFail: () => void }) => {
	const { width, height } = useWindowDimensions();
	return (
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
				accessibilityLabel={label}
				source={{ uri: url }}
				resizeMode="contain"
				onError={onFail}
				style={{ width, height }}
			/>
		</ScrollView>
	);
};

// Why the viewer can't show a picture, and what you can do about it.
const WholeNote = ({ text, onRetry }: { text: string; onRetry?: () => void }) => {
	const { t } = useI18n();
	return (
		<View accessibilityRole="alert" style={styles.wholeNote}>
			<Text style={styles.wholeNoteText}>{text}</Text>
			{onRetry ? <Button small label={t("Try again")} onPress={onRetry} /> : null}
		</View>
	);
};

// A picture on the agent's device, fetched whole when the viewer opens.
const DeviceWhole = ({
	agent,
	path,
	name,
	thumbnail,
}: {
	agent: AgentId;
	path: string;
	name: string;
	thumbnail: string | null;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const media = useMedia(agent, path);
	useEffect(() => {
		void loadMedia(source, agent, path, FULL_LIMIT);
	}, [source, agent, path]);

	switch (media?.state) {
		case "ready":
			return (
				<Zoomed
					url={media.url}
					label={name}
					onFail={() =>
						void mediaLost(source, agent, path, t("Couldn't show {{name}}.", { name }))
					}
				/>
			);
		case "failed":
			return (
				<WholeNote
					text={media.reason}
					onRetry={() => void loadMedia(source, agent, path, FULL_LIMIT)}
				/>
			);
		case "large":
			return (
				<WholeNote
					text={t("{{name}} is {{size}}, too large to show here.", {
						name,
						size: formatBytes(media.size),
					})}
				/>
			);
		default: {
			const done = media?.state === "loading" ? percent(media.sent, media.total) : null;
			return (
				<View style={[styles.fill, styles.center]}>
					{thumbnail ? (
						<Image
							source={{ uri: thumbnail }}
							resizeMode="contain"
							style={[StyleSheet.absoluteFill, styles.faded]}
						/>
					) : null}
					<View
						accessible
						accessibilityRole="progressbar"
						accessibilityLabel={t("Loading {{name}}", { name })}
						style={styles.progress}
					>
						<Spinner />
						{done !== null ? (
							<Text style={styles.progressText}>
								{formatNumber(done / 100, { style: "percent" })}
							</Text>
						) : null}
						<Button
							small
							label={t("Cancel")}
							onPress={() => cancelMedia(source, agent, path)}
						/>
					</View>
				</View>
			);
		}
	}
};

// A picture the app can load now. One that won't draw says so.
const UrlWhole = ({ url, name }: { url: string; name: string }) => {
	const { t } = useI18n();

	const [failed, setFailed] = useState(false);
	return failed ? (
		<WholeNote text={t("Couldn't show {{name}}.", { name })} />
	) : (
		<Zoomed url={url} label={name} onFail={() => setFailed(true)} />
	);
};

/** The picture shown whole, over the whole app. Mount it once. */
export const PictureViewer = () => {
	const { t } = useI18n();

	const whole = useStore(shown, (s) => s.whole);
	const insets = useSafeAreaInsets();
	if (!whole) return null;
	return (
		<Modal visible transparent animationType="fade" onRequestClose={closeWhole}>
			<View accessibilityViewIsModal onAccessibilityEscape={closeWhole} style={styles.scrim}>
				{whole.kind === "url" ? (
					<UrlWhole key={whole.url} url={whole.url} name={whole.name} />
				) : (
					<DeviceWhole
						key={`${whole.agent}\n${whole.path}`}
						agent={whole.agent}
						path={whole.path}
						name={whole.name}
						thumbnail={whole.thumbnail}
					/>
				)}
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={t("Close")}
					onPress={closeWhole}
					hitSlop={8}
					style={[styles.close, { top: insets.top + 8 }]}
				>
					<Close tint={color.foregroundStrong} />
				</Pressable>
			</View>
		</Modal>
	);
};

const styles = StyleSheet.create({
	fill: { flex: 1 },
	center: { flexGrow: 1, alignItems: "center", justifyContent: "center" },
	frame: {
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		overflow: "hidden",
	},
	waiting: {
		width: "100%",
		maxWidth: MOST,
		aspectRatio: 16 / 9,
		flexDirection: "row",
		alignItems: "center",
		justifyContent: "center",
		gap: 6,
		backgroundColor: color.subpanel,
	},
	percent: { color: color.mutedForeground, fontSize: size.tiny, fontVariant: ["tabular-nums"] },
	note: {
		alignSelf: "flex-start",
		maxWidth: "100%",
		gap: 8,
		paddingHorizontal: 10,
		paddingVertical: 8,
		borderRadius: radius.sm,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.border,
		backgroundColor: color.subpanel,
	},
	noteText: { color: color.mutedForeground, fontSize: size.meta },
	more: { alignSelf: "flex-start" },
	thumbs: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
	thumb: {
		width: 176,
		height: 94,
		alignItems: "center",
		justifyContent: "center",
		overflow: "hidden",
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.canvas,
	},
	strip: {
		position: "absolute",
		left: 0,
		right: 0,
		bottom: 0,
		paddingHorizontal: 8,
		paddingVertical: 3,
		backgroundColor: "rgba(0, 0, 0, 0.6)",
	},
	stripText: { color: color.terminal, fontSize: size.tiny },
	thumbNote: {
		paddingHorizontal: 10,
		paddingBottom: 18,
		color: color.mutedForeground,
		fontSize: size.tiny,
		textAlign: "center",
	},
	number: {
		position: "absolute",
		top: 4,
		left: 4,
		paddingHorizontal: 6,
		paddingVertical: 1,
		borderRadius: 4,
		backgroundColor: "rgba(0, 0, 0, 0.6)",
	},
	numberText: { color: color.terminal, fontSize: 10.5, fontWeight: "500" },
	scrim: { flex: 1, backgroundColor: "#000000" },
	close: {
		position: "absolute",
		right: 12,
		width: 36,
		height: 36,
		alignItems: "center",
		justifyContent: "center",
		borderRadius: 18,
		backgroundColor: tint("#000000", 0.55),
	},
	faded: { opacity: 0.5 },
	progress: {
		flexDirection: "row",
		alignItems: "center",
		gap: 10,
		paddingLeft: 12,
		paddingRight: 8,
		paddingVertical: 8,
		borderRadius: radius.md,
		backgroundColor: "rgba(0, 0, 0, 0.6)",
	},
	progressText: { color: color.terminal, fontSize: size.meta, fontVariant: ["tabular-nums"] },
	wholeNote: { flex: 1, alignItems: "center", justifyContent: "center", gap: 12, padding: 32 },
	wholeNoteText: { color: color.terminal, fontSize: size.small, textAlign: "center" },
});
