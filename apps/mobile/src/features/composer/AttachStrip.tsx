// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The photos and files the paperclip added, in a row above the field, each
// with its picture or a file's mark, its name, a spinner while it gets
// ready on the agent's device, and an X that takes it out. One that
// couldn't get ready shows in red with why.

import { useI18n } from "@maxhq/shared/i18n";

import { previewOf } from "@maxhq/shared/agent-view/pictures";
import type { StagedFile } from "@maxhq/shared/composer/staging";
import { fileLabel, middleTruncate } from "@maxhq/shared/pills/label";
import { memo } from "react";
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";

import { color, radius, size, TOUCH } from "../../theme";
import { Spinner } from "../../ui/Dot";
import { File as FileGlyph } from "../../ui/glyphs";
import { Close } from "../../ui/Icon";

const FACE = 36;

const Staged = memo(
	({
		file,
		number,
		disabled,
		onRemove,
	}: {
		file: StagedFile;
		/** Its number among the pasted pictures, for its label. */
		number: number | undefined;
		disabled: boolean;
		onRemove: (key: string) => void;
	}) => {
		const { t } = useI18n();
		const name = middleTruncate(
			fileLabel(
				{ name: file.ref?.name ?? file.name, kind: file.ref?.kind ?? file.kind },
				number
			)
		);
		const face = (file.ref && previewOf(file.ref)) ?? file.preview;
		const failed = file.state === "failed";
		return (
			<View style={[styles.file, failed && styles.failedFile]}>
				{face ? (
					<Image
						source={{ uri: face }}
						style={styles.face}
						accessibilityIgnoresInvertColors
					/>
				) : (
					<View style={[styles.face, styles.mark]}>
						<FileGlyph size={16} strokeWidth={1.8} color={color.mutedForeground} />
					</View>
				)}
				<View style={styles.words}>
					<Text style={styles.name} numberOfLines={1}>
						{name}
					</Text>
					{failed ? (
						<Text style={styles.error} numberOfLines={2}>
							{file.error ?? t("Couldn't get it ready")}
						</Text>
					) : null}
				</View>
				{file.state === "staging" ? <Spinner tone={color.mutedForeground} /> : null}
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={t("Remove {{name}}", { name })}
					accessibilityState={{ disabled }}
					disabled={disabled}
					onPress={() => onRemove(file.key)}
					hitSlop={(TOUCH - 28) / 2}
					style={({ pressed }) => [styles.remove, pressed && styles.pressed]}
				>
					<Close size={12} />
				</Pressable>
			</View>
		);
	}
);
Staged.displayName = "Staged";

/** The files in the strip, in the order you added them. */
export const AttachStrip = ({
	files,
	numbers,
	disabled,
	onRemove,
}: {
	files: readonly StagedFile[];
	numbers: ReadonlyMap<string, number>;
	/** Whether the X's wait, such as while the message sends. */
	disabled: boolean;
	onRemove: (key: string) => void;
}) => {
	if (files.length === 0) return null;
	return (
		<ScrollView
			horizontal
			showsHorizontalScrollIndicator={false}
			keyboardShouldPersistTaps="handled"
			contentContainerStyle={styles.row}
		>
			{files.map((file) => (
				<Staged
					key={file.key}
					file={file}
					number={numbers.get(file.key)}
					disabled={disabled}
					onRemove={onRemove}
				/>
			))}
		</ScrollView>
	);
};

const styles = StyleSheet.create({
	row: { gap: 6 },
	file: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		maxWidth: 220,
		paddingLeft: 4,
		paddingRight: 4,
		paddingVertical: 4,
		borderRadius: radius.md,
		borderWidth: StyleSheet.hairlineWidth,
		borderColor: color.input,
		backgroundColor: color.secondary,
	},
	failedFile: { borderColor: color.destructive },
	face: { width: FACE, height: FACE, borderRadius: radius.sm },
	mark: { alignItems: "center", justifyContent: "center", backgroundColor: color.accent },
	words: { flexShrink: 1, minWidth: 0 },
	name: { color: color.foreground, fontSize: size.meta },
	error: { color: color.errorText, fontSize: size.tiny },
	remove: {
		width: 28,
		height: 28,
		borderRadius: 14,
		alignItems: "center",
		justifyContent: "center",
	},
	pressed: { backgroundColor: color.accent },
});
