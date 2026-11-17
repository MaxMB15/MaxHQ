// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Exports an agent's whole conversation as Markdown, as the desktop's More
// menu does. It loads the older pages first, with a line under the header
// while it does, then copies the text or saves it as a file: through the
// share sheet on iOS, where Save to Files is, and into a folder you pick on
// Android, whose share sheet takes no files. An alert says how it went.

import { t, useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { Directory, File, Paths } from "expo-file-system";
import { useRef, useState } from "react";
import { Alert, Platform, Share, StyleSheet, Text, View } from "react-native";

import { viewNames } from "@maxhq/shared/agent-view/agent-tools";
import type { Agent } from "@maxhq/shared/bindings";
import { exportFileName } from "@maxhq/shared/timeline/export";
import { conversationText } from "@maxhq/shared/timeline/whole";

import { useServices } from "../../services";
import { color, size } from "../../theme";
import { Spinner } from "../../ui/Dot";

/** Where the conversation goes: a file, or the clipboard. */
export type ExportTo = "file" | "copy";

// The words of an error something threw.
const said = (error: unknown): string =>
	error instanceof Error && error.message ? error.message : t("It didn't load.");

// Saves `text` as a Markdown file named `name`. Resolves to whether it
// should say so, which the share sheet does on its own.
const saveFile = async (name: string, text: string): Promise<boolean> => {
	if (Platform.OS === "ios") {
		const file = new File(Paths.cache, name);
		await file.write(text);
		await Share.share({ url: file.uri });
		return false;
	}
	const folder = await Directory.pickDirectoryAsync().catch(() => null);
	// Closed without picking one.
	if (!folder) return false;
	await folder.createFile(name, "text/markdown").write(text);
	return true;
};

/**
 * Exports the agent's conversation: `loading` while the older pages load,
 * and `run` to start one. A second run waits for the first.
 */
export const useConversationExport = (
	agent: Pick<Agent, "id" | "title" | "vendor">
): { readonly loading: boolean; readonly run: (to: ExportTo) => void } => {
	const services = useServices();
	const [loading, setLoading] = useState(false);
	const busy = useRef(false);

	const go = async (to: ExportTo) => {
		let text: string;
		setLoading(true);
		try {
			const names = viewNames(services.view.getState().view);
			text = await conversationText(services.source, agent, names);
		} catch (error) {
			Alert.alert(t("Couldn't export"), said(error));
			return;
		} finally {
			setLoading(false);
		}
		try {
			if (to === "copy") {
				await Clipboard.setStringAsync(text);
				Alert.alert(
					t("Copied as Markdown"),
					t("The whole conversation is on the clipboard.")
				);
				return;
			}
			const name = exportFileName(agent.title, new Date());
			if (await saveFile(name, text)) Alert.alert(t("Saved as Markdown"), name);
		} catch (error) {
			Alert.alert(t("Couldn't export"), said(error));
		}
	};

	const run = (to: ExportTo) => {
		if (busy.current) return;
		busy.current = true;
		void go(to).finally(() => {
			busy.current = false;
		});
	};
	return { loading, run };
};

/** The line under the header while the older pages load. */
export const ExportBar = () => {
	const { t } = useI18n();
	return (
		<View style={styles.bar} accessibilityLiveRegion="polite">
			<Spinner size={11} />
			<Text style={styles.text}>{t("Loading earlier messages")}</Text>
		</View>
	);
};

const styles = StyleSheet.create({
	bar: {
		flexDirection: "row",
		alignItems: "center",
		gap: 8,
		paddingHorizontal: 14,
		paddingVertical: 6,
		borderBottomWidth: StyleSheet.hairlineWidth,
		borderBottomColor: color.border,
	},
	text: { color: color.mutedForeground, fontSize: size.meta },
});
