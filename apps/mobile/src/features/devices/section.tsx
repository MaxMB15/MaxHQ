// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What every section of a device page shares: the props each takes, a
// padded body for a card's text and buttons, a row of buttons, a line that
// says how an action went, and a hook that runs one action at a time and
// keeps that line.

import * as Clipboard from "expo-clipboard";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { StyleSheet, Text, View } from "react-native";

import type { DeviceId } from "@maxhq/shared/bindings";
import { failureOf } from "@maxhq/shared/data/failure";
import type { ActionResult, CommandResult } from "@maxhq/shared/data/source";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";
import type { ViewState } from "@maxhq/shared/view/replica";

import { color, size } from "../../theme";
import { Button } from "../../ui/Button";

/** What a device page hands each of its sections. */
export type SectionProps = {
	readonly device: DeviceId;
	readonly name: string;
	/** Whether the device's maxd answers now. */
	readonly live: boolean;
	readonly view: ViewState;
};

/** A short line under an action: how it went, or why it didn't. */
export type Note = { readonly text: string; readonly error: boolean };

/** A card's text and buttons, padded as its rows are. */
export const CardBody = ({ children, first }: { children: ReactNode; first?: boolean }) => (
	<View style={[styles.body, !first && styles.line]}>{children}</View>
);

/** Buttons side by side, wrapping when they don't fit. */
export const Buttons = ({ children }: { children: ReactNode }) => (
	<View style={styles.buttons}>{children}</View>
);

export const NoteLine = ({ note }: { note: Note | null }) =>
	note ? (
		<Text accessibilityLiveRegion="polite" style={[styles.note, note.error && styles.error]}>
			{note.text}
		</Text>
	) : null;

/** A small button that copies `text`, and says for a moment that it did. */
export const CopyTextButton = ({ label, text }: { label: string; text: string }) => {
	const [state, copy] = useCopyWith(Clipboard.setStringAsync);
	return <Button label={copyLabel(state, label)} small onPress={() => copy(text)} />;
};

/** A line of muted text, such as a card's empty state. */
export const Muted = ({ children }: { children: ReactNode }) => (
	<Text style={styles.muted}>{children}</Text>
);

/**
 * Runs one action at a time. `run` shows `pending` while it waits, then
 * why it failed, or `done` when it went through, and gives the result back
 * when it went through, or null.
 */
export const useAction = () => {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const alive = useRef(true);
	useEffect(() => {
		alive.current = true;
		return () => {
			alive.current = false;
		};
	}, []);
	const run = async <R extends ActionResult | CommandResult>(
		pending: string | null,
		call: () => Promise<R>,
		done?: string | null
	): Promise<R | null> => {
		setBusy(true);
		setNote(pending ? { text: pending, error: false } : null);
		try {
			const result = await call();
			const why = failureOf(result);
			if (alive.current)
				setNote(
					why ? { text: why, error: true } : done ? { text: done, error: false } : null
				);
			return why === null ? result : null;
		} catch (error) {
			if (alive.current)
				setNote({
					text: error instanceof Error ? error.message : String(error),
					error: true,
				});
			return null;
		} finally {
			if (alive.current) setBusy(false);
		}
	};
	return { busy, note, setNote, run };
};

const styles = StyleSheet.create({
	body: { paddingHorizontal: 14, paddingVertical: 12, gap: 8 },
	line: { borderTopColor: color.border, borderTopWidth: StyleSheet.hairlineWidth },
	buttons: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
	note: { color: color.mutedForeground, fontSize: size.meta },
	error: { color: color.errorText },
	muted: { color: color.mutedForeground, fontSize: size.small },
});
