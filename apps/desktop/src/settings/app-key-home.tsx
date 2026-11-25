// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where this computer's app key is kept, on Linux and Windows. When no
// keyring answers as the app opens, a dialog asks before the key goes in a
// file, and says what that risks. Not now keeps the key in memory until
// MaxHQ quits, and the app doesn't ask again. The App keys card shows where
// the key is and moves it between the keyring and the file, asking the same
// question before it goes in a file. A Mac keeps its key in the Keychain,
// so none of this shows there.

import { AlertDialog } from "@base-ui/react/alert-dialog";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { commands } from "@/data/tauri-commands";
import { cn } from "@/lib/utils";
import { unwrap } from "@/native/failure";
import { inApp } from "@/native/platform";
import { BACKDROP, POPUP } from "@/workspaces/new-workspace";
import type { KeyHome, KeyHomeState } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { PRIMARY, Row, SECONDARY, StatusNote } from "./controls";
import type { Note } from "./result";

const KEY_HOME = "app-key-home";

/** The App keys card's query, which shows what the key's store did. */
const APP_KEY = "app-key";

/** Where the key is kept, or null on a Mac and in a browser. */
const useKeyHome = (): KeyHomeState | null => {
	const result = useQuery({
		queryKey: [KEY_HOME],
		queryFn: () => (inApp() ? commands.appKeyHome() : Promise.resolve(null)),
	});
	return result.data ?? null;
};

/** Moves the key and tells both queries where it went. */
const useMove = () => {
	const queries = useQueryClient();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	/** Resolves with whether the key moved. */
	const move = async (home: KeyHome): Promise<boolean> => {
		setBusy(true);
		setNote(null);
		try {
			queries.setQueryData([KEY_HOME], await unwrap(commands.keepAppKeyIn(home)));
			return true;
		} catch (error) {
			console.warn("couldn't move the app key", error);
			setNote({
				text:
					error instanceof Error
						? error.message
						: translate("Couldn't do it. Try again."),
				error: true,
			});
			return false;
		} finally {
			setBusy(false);
			await queries.invalidateQueries({ queryKey: [APP_KEY] });
		}
	};
	return { busy, note, move };
};

/** The question before the key goes in a file: why, what it risks, where the file goes. */
const FileQuestion = ({
	open,
	title,
	why,
	file,
	decline,
	hint,
	busy,
	note,
	onDecline,
	onKeep,
}: {
	open: boolean;
	title: string;
	why: string;
	file: string;
	decline: string;
	hint?: string;
	busy: boolean;
	note: Note | null;
	onDecline: () => void;
	onKeep: () => void;
}) => {
	const { t } = useI18n();
	return (
		<AlertDialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next && !busy) onDecline();
			}}
		>
			<AlertDialog.Portal>
				<AlertDialog.Backdrop className={BACKDROP} />
				<AlertDialog.Popup className={cn(POPUP, "w-[min(460px,calc(100vw-32px))]")}>
					<div className="flex flex-col gap-4">
						<div className="flex flex-col gap-2 text-[12.5px] text-muted-foreground">
							<AlertDialog.Title className="text-[15px] font-[650] text-foreground-strong">
								{title}
							</AlertDialog.Title>
							<AlertDialog.Description>{why}</AlertDialog.Description>
							<p>
								{t(
									"Only your user account can read the file, but every program you run can read it too, and so can anyone who gets into your account. Whoever copies it can prove to your devices that they are this computer."
								)}
							</p>
							<p>
								{t("The file goes here:")}{" "}
								<span className="selectable font-mono text-[11.5px] break-all text-foreground">
									{file}
								</span>
							</p>
							{hint ? <p>{hint}</p> : null}
							<StatusNote note={note} />
						</div>
						<div className="flex items-center justify-end gap-2">
							<button
								type="button"
								className={SECONDARY}
								disabled={busy}
								onClick={onDecline}
							>
								{decline}
							</button>
							<button
								type="button"
								className={PRIMARY}
								disabled={busy}
								onClick={onKeep}
							>
								{t("Keep it in a file")}
							</button>
						</div>
					</div>
				</AlertDialog.Popup>
			</AlertDialog.Portal>
		</AlertDialog.Root>
	);
};

/** The question the app asks once when no keyring answered as it opened. */
export const AppKeyHomePrompt = () => {
	const { t } = useI18n();
	const state = useKeyHome();
	const { busy, note, move } = useMove();
	// Not now closes the dialog at once, even when saving the answer fails,
	// since the key stays in memory either way.
	const [answered, setAnswered] = useState(false);
	if (state?.file == null) return null;
	return (
		<FileQuestion
			open={state.ask && !answered}
			title={t("Where should MaxHQ keep its app key?")}
			why={t(
				"No keyring answered as MaxHQ opened, so it has nowhere safe to keep this computer's app key. Without one, the key lasts until MaxHQ quits, and each time it opens it makes a new key your devices don't know yet."
			)}
			file={state.file}
			decline={t("Not now")}
			hint={t("You can change this later in Settings, on the App keys card.")}
			busy={busy}
			note={note}
			onDecline={() => {
				setAnswered(true);
				void move("memory");
			}}
			onKeep={() => void move("file")}
		/>
	);
};

/** Where the key is kept, in this computer's part of the App keys card, with moves between the keyring and the file. */
export const AppKeyHomeRow = () => {
	const { t } = useI18n();
	const state = useKeyHome();
	const { busy, note, move } = useMove();
	const [asking, setAsking] = useState(false);
	if (state === null) return null;
	const { home, file } = state;
	const sub =
		home === "keyring"
			? t("In the system keyring.")
			: home === "file"
				? t("In a file only your account can read: {{path}}", { path: file ?? "" })
				: t("Nowhere, so the key lasts until MaxHQ quits.");
	return (
		<Row title={t("Where the key is kept")} sub={<span className="selectable">{sub}</span>}>
			<StatusNote note={asking ? null : note} />
			{home !== "keyring" ? (
				<button
					type="button"
					className={SECONDARY}
					disabled={busy}
					onClick={() => void move("keyring")}
				>
					{t("Move to the keyring")}
				</button>
			) : null}
			{home !== "file" && file !== null ? (
				<>
					<button
						type="button"
						className={SECONDARY}
						disabled={busy}
						onClick={() => setAsking(true)}
					>
						{t("Keep it in a file")}
					</button>
					<FileQuestion
						open={asking}
						title={t("Keep the app key in a file?")}
						why={
							home === "keyring"
								? t(
										"The key moves out of the system keyring, which keeps it locked while you're signed out, and into a file."
									)
								: t(
										"No keyring answered as MaxHQ opened, so it has nowhere safe to keep this computer's app key. Without one, the key lasts until MaxHQ quits, and each time it opens it makes a new key your devices don't know yet."
									)
						}
						file={file}
						decline={t("Cancel")}
						busy={busy}
						note={note}
						onDecline={() => setAsking(false)}
						onKeep={() =>
							void move("file").then((moved) => {
								if (moved) setAsking(false);
							})
						}
					/>
				</>
			) : null}
		</Row>
	);
};
