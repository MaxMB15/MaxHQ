// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What this window knows about the app's own update, and the words for it.
// The app holds the status and says when it changes. The window adds only
// Later, which hides the prompt for that version until MaxHQ starts again.
// The settings page still shows the update.

import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { formatDate, formatNumber, t } from "@maxhq/shared/i18n";
import {
	type AppUpdate,
	appUpdate,
	checkForUpdate,
	installUpdate,
	type NoUpdates,
	onUpdateChanged,
	setUpdatePrefs,
	type UpdateStatus,
} from "@/native/updates";

/** The window's view of the update. */
export type UpdateState = {
	/** What the app said, once it has. */
	readonly update: AppUpdate | null;
	/** The version you said Later to. */
	readonly later: string | null;
	/** Why the last thing you asked for failed, when the status doesn't say. */
	readonly error: string | null;
};

const EMPTY: UpdateState = { update: null, later: null, error: null };

/** The window's update state. Each window has its own, since Later is per window. */
export const updates = createStore<UpdateState>(() => EMPTY);

const said = (error: unknown): string => (error instanceof Error ? error.message : String(error));

const setStatus = (status: UpdateStatus) =>
	updates.setState((s) => (s.update ? { update: { ...s.update, status } } : {}));

let watchers = 0;
let stop: (() => void) | null = null;

/**
 * Reads the update from the app and follows its changes while anything
 * watches. Returns a function that stops watching.
 */
export const watchUpdates = (): (() => void) => {
	watchers += 1;
	if (watchers === 1) {
		let live = true;
		const listening = onUpdateChanged((status) => live && setStatus(status));
		// Read once it listens, so no change falls between the two.
		void listening
			.then(appUpdate)
			.then((update) => live && updates.setState({ update }))
			.catch((error: unknown) => live && updates.setState({ error: said(error) }));
		stop = () => {
			live = false;
			void listening.then((unlisten) => unlisten()).catch(() => undefined);
		};
	}
	return () => {
		watchers -= 1;
		if (watchers === 0) {
			stop?.();
			stop = null;
		}
	};
};

/** The window's update state, followed while the component shows. */
export const useUpdates = <T>(selector: (state: UpdateState) => T): T => {
	useEffect(() => watchUpdates(), []);
	return useStore(updates, selector);
};

/** Looks for a newer version now. */
export const checkNow = async (): Promise<void> => {
	updates.setState({ error: null });
	try {
		setStatus(await checkForUpdate());
	} catch (error) {
		updates.setState({ error: said(error) });
	}
};

/** Downloads and installs the version found. The app restarts once it's in. */
export const updateNow = async (): Promise<void> => {
	updates.setState({ error: null, later: null });
	try {
		await installUpdate();
	} catch (error) {
		// The status says why too; this covers a call that never reached it.
		updates.setState({ error: said(error) });
	}
};

/** Hides the prompt for this version until MaxHQ starts again. */
export const later = (version: string): void => updates.setState({ later: version });

/** Turns looking for updates on its own on or off. */
export const setAutoCheck = async (on: boolean): Promise<void> => {
	const update = updates.getState().update;
	if (!update) return;
	const prefs = { ...update.prefs, auto_check: on };
	updates.setState({ update: { ...update, prefs }, error: null });
	try {
		await setUpdatePrefs(prefs);
	} catch (error) {
		updates.setState((s) => ({
			update: s.update ? { ...s.update, prefs: update.prefs } : null,
			error: t("Not saved: {{said}}", { said: said(error) }),
		}));
	}
};

/** The version the prompt is about, or null when it doesn't show. */
export const promptVersion = (state: UpdateState): string | null => {
	const status = state.update?.status;
	if (!status) return null;
	// A failed check has no version, so only a failed install prompts.
	const version =
		status.state === "available" ||
		status.state === "downloading" ||
		status.state === "installing" ||
		status.state === "failed"
			? status.version
			: null;
	return version !== null && version !== state.later ? version : null;
};

/** Whether the app is downloading or installing an update. */
export const installing = (status: UpdateStatus | undefined): boolean =>
	status?.state === "downloading" || status?.state === "installing";

/** How much of the download is in, from 0 to 1, when the size is known. */
export const progressOf = (status: UpdateStatus | undefined): number | null => {
	if (status?.state === "installing") return 1;
	if (status?.state !== "downloading" || !status.total) return null;
	return Math.min(1, status.downloaded / status.total);
};

/** Why this copy of the app doesn't update itself, in plain words. */
export const unavailableText = (reason: NoUpdates): string => {
	switch (reason) {
		case "from_source":
			return t(
				"This copy was built from source, so it doesn't update itself. Build the new version the same way."
			);
		case "disk_image":
			return t(
				"MaxHQ is running from its disk image. Drag it to Applications and open it from there to get updates."
			);
		case "translocated":
			return t(
				"macOS is running a temporary copy of MaxHQ, since it was opened where it was downloaded. Move it to Applications and open it from there to get updates."
			);
		case "not_in_applications":
			return t(
				"MaxHQ updates itself only from the Applications folder. Move it there to get updates."
			);
	}
};

/** Where the update stands, in a line. */
export const statusText = (status: UpdateStatus): string => {
	switch (status.state) {
		case "unavailable":
			return unavailableText(status.reason);
		case "idle":
			return t("MaxHQ hasn't checked for updates yet.");
		case "checking":
			return t("Checking for updates…");
		case "up_to_date":
			return t("MaxHQ is up to date. Last checked at {{time}}.", {
				time: formatDate(status.checked_at, { timeStyle: "short" }),
			});
		case "available":
			return t("Version {{version}} is available.", { version: status.version });
		case "downloading": {
			const progress = progressOf(status);
			return progress === null
				? t("Downloading version {{version}}…", { version: status.version })
				: t("Downloading version {{version}}… {{percent}}", {
						version: status.version,
						percent: formatNumber(progress, { style: "percent" }),
					});
		}
		case "installing":
			return t("Installing version {{version}}…", { version: status.version });
		case "failed":
			return status.version === null
				? t("Couldn't check for updates: {{reason}}", { reason: status.reason })
				: t("Couldn't install version {{version}}: {{reason}}", {
						version: status.version,
						reason: status.reason,
					});
	}
};

/** The page that says what changed in `version`. */
export const releaseNotesUrl = (
	releases: string | null | undefined,
	version: string
): string | null => (releases ? `${releases}/tag/v${version}` : null);

/** Forgets everything, for tests. */
export const resetUpdates = (): void => {
	stop?.();
	stop = null;
	watchers = 0;
	updates.setState(EMPTY, true);
};
