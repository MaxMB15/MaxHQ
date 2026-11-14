// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What shows at the bottom of the window after Add to note or a drop on a
// note: "Added to" the note, with Open and Undo, for a few seconds. Undo
// that can't, since the block changed since, says so in its place. It
// draws in a root of its own on the page, so it shows whatever opened it.

import { useI18n } from "@maxhq/shared/i18n";

import { useSyncExternalStore } from "react";
import { createRoot, type Root } from "react-dom/client";

/** One addition the toast offers to open or undo. */
export type Added = {
	/** The note's title as the sidebar shows it. */
	readonly note: string;
	readonly open: () => void;
	/** Takes the block out again. Resolves with why it couldn't, or null. */
	readonly undo: () => Promise<string | null>;
};

type Showing =
	| { readonly kind: "added"; readonly added: Added }
	| { readonly kind: "said"; readonly text: string };

/** How long the toast stays. */
export const ADDED_MS = 6_000;

let showing: Showing | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let root: Root | null = null;
const listeners = new Set<() => void>();

const set = (next: Showing | null) => {
	showing = next;
	if (timer !== null) clearTimeout(timer);
	timer = next ? setTimeout(() => set(null), ADDED_MS) : null;
	for (const listener of [...listeners]) listener();
};

const subscribe = (listener: () => void) => {
	listeners.add(listener);
	return () => {
		listeners.delete(listener);
	};
};

/** Shows the toast for a block just added, or a sentence such as why it wasn't. */
export const showAdded = (next: Added | string): void => {
	if (!root) {
		const host = document.createElement("div");
		host.dataset.addedToast = "";
		document.body.append(host);
		root = createRoot(host);
		root.render(<AddedToast />);
	}
	set(typeof next === "string" ? { kind: "said", text: next } : { kind: "added", added: next });
};

/** Hides the toast, as its time running out does. */
export const hideAdded = (): void => set(null);

const BUTTON =
	"h-[24px] shrink-0 rounded-[5px] px-2 text-[12px] font-[550] text-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring";

const AddedToast = () => {
	const { t } = useI18n();
	const now = useSyncExternalStore(subscribe, () => showing);
	return (
		<div
			role="status"
			className="pointer-events-none fixed inset-x-0 bottom-5 z-[140] flex justify-center px-4"
		>
			{now ? (
				<div className="pointer-events-auto flex h-[36px] max-w-full items-center gap-1.5 rounded-[9px] border border-border bg-background pr-1 pl-3 text-[12.5px] text-foreground shadow-lg">
					{now.kind === "said" ? (
						<span className="min-w-0 truncate pr-2">{now.text}</span>
					) : (
						<>
							<span className="shrink-0">{t("Added to")}</span>
							<span className="min-w-0 truncate rounded-[5px] border border-(--pill-note-border) bg-(--pill-note-bg) px-1.5 text-(--pill-note-text)">
								{now.added.note}
							</span>
							<button
								type="button"
								className={BUTTON}
								onClick={() => {
									hideAdded();
									now.added.open();
								}}
							>
								{t("Open")}
							</button>
							<button
								type="button"
								className={BUTTON}
								onClick={() => {
									hideAdded();
									void now.added.undo().then((why) => {
										if (why) showAdded(why);
									});
								}}
							>
								{t("Undo")}
							</button>
						</>
					)}
				</div>
			) : null}
		</div>
	);
};
