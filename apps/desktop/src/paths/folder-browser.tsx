// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// MaxHQ's own folder browser, for a device the system picker can't see. It
// lists one folder at a time through that device's maxd: a breadcrumb with
// an up button, checkouts tagged with their branch, Show hidden, New folder,
// and Choose with the chosen path in the footer. A path field opens it from
// Browse…, and anything else that wants a folder on a device can too. What
// it does lives in @maxhq/shared/folders/browser; this draws it.

import { Dialog } from "@base-ui/react/dialog";
import { ArrowUp, ChevronRight, File, Folder } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { useServices, useView } from "@/app/services";
import { Spinner } from "@/agent-view/icons";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { FIELD, PRIMARY, SECONDARY } from "@/settings/controls";
import type { DeviceId, FolderEntry } from "@maxhq/shared/bindings";
import { crumbsOf, type FolderBrowser, useFolderBrowser } from "@maxhq/shared/folders/browser";
import { checkoutTag, joinPath, type PathWants } from "@maxhq/shared/folders/path";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { deviceName } from "@maxhq/shared/view/select";

const ROW =
	"flex h-7 w-full cursor-default items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px] text-foreground select-none";

const ICON = "size-3.5 shrink-0 text-muted-foreground";

/** A checkout's tag, as the path field's suggestions show it too. */
export const CheckoutTag = ({ entry }: { entry: FolderEntry }) => {
	const tag = checkoutTag(entry);
	return tag === null ? null : (
		<span className="shrink-0 rounded-[4px] border border-status-ready/40 px-1.5 text-[10.5px] leading-4 text-status-ready-text">
			{tag}
		</span>
	);
};

/** The line where you name a new folder, with why it couldn't be made. */
const NewFolder = ({ browser, onDone }: { browser: FolderBrowser; onDone: () => void }) => {
	useI18n();
	const [name, setName] = useState("");
	const [problem, setProblem] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const make = async () => {
		setBusy(true);
		const why = await browser.make(name);
		setBusy(false);
		if (why === null) onDone();
		else setProblem(why);
	};
	return (
		<div className="flex flex-col gap-1">
			<div className="flex items-center gap-2">
				<Folder aria-hidden className={ICON} />
				<input
					autoFocus
					aria-label={translate("New folder's name")}
					value={name}
					disabled={busy}
					spellCheck={false}
					autoComplete="off"
					onChange={(e) => {
						setName(e.target.value);
						setProblem(null);
					}}
					onKeyDown={(e) => {
						if (e.key === "Enter") {
							e.preventDefault();
							void make();
						} else if (e.key === "Escape") {
							e.preventDefault();
							e.stopPropagation();
							onDone();
						}
					}}
					className={cn(FIELD, "h-7 font-mono text-xs")}
				/>
			</div>
			{problem ? (
				<p role="alert" className="pl-5.5 text-[11.5px] text-status-error-text">
					{problem}
				</p>
			) : null}
		</div>
	);
};

/** The browser's insides, mounted while it's open. */
const Browser = ({
	device,
	start,
	wants,
	onChoose,
}: {
	device: DeviceId;
	start: string;
	wants: PathWants;
	onChoose: (path: string) => void;
}) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const browser = useFolderBrowser({ calls: services.source, device, start, wants });
	const [naming, setNaming] = useState(false);
	const list = useRef<HTMLDivElement>(null);
	const { listing, selected, chosen } = browser;
	const entries = listing?.entries ?? [];
	const name = deviceName(view, device);

	useEffect(() => {
		if (selected === null) return;
		list.current
			?.querySelector(`[data-name="${CSS.escape(selected)}"]`)
			?.scrollIntoView?.({ block: "nearest" });
	}, [selected, listing]);

	const choose = () => {
		if (chosen !== null) onChoose(chosen);
	};

	// ↑ and ↓ pick, → opens a folder, ← goes up and ↩ chooses.
	const keys = (event: KeyboardEvent) => {
		const at = entries.findIndex((e) => e.name === selected);
		const entry = at >= 0 ? entries[at] : undefined;
		// With nothing picked, ↓ picks the first and ↑ the last.
		const step = (by: number) => {
			const next = at < 0 ? (by > 0 ? 0 : entries.length - 1) : at + by;
			const to = entries[Math.min(entries.length - 1, Math.max(0, next))];
			if (to) browser.select(to.name);
		};
		const act: Record<string, () => void> = {
			ArrowDown: () => step(1),
			ArrowUp: () => step(-1),
			ArrowRight: () => {
				if (entry?.folder) browser.open(entry);
			},
			ArrowLeft: browser.up,
			Enter: choose,
		};
		const run = act[event.key];
		if (!run) return;
		event.preventDefault();
		run();
	};

	return (
		<div className="flex min-h-0 flex-col gap-3">
			<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
				{wants === "folder"
					? translate("Choose a folder on {{device}}", { device: name })
					: translate("Choose a file on {{device}}", { device: name })}
			</Dialog.Title>
			<div className="flex min-w-0 items-center gap-1">
				<button
					type="button"
					aria-label={translate("Up")}
					disabled={listing === null || listing.path === "/"}
					onClick={browser.up}
					className="grid size-6 shrink-0 cursor-pointer place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:cursor-default disabled:opacity-40 disabled:hover:bg-transparent"
				>
					<ArrowUp aria-hidden className="size-3.5" />
				</button>
				<nav
					aria-label={translate("Folders above")}
					className="flex min-w-0 flex-1 items-center overflow-x-auto font-mono text-xs whitespace-nowrap"
				>
					{listing
						? crumbsOf(listing.path).map((crumb, i, all) => (
								<span key={crumb.path} className="flex items-center">
									{i > 1 ? (
										<span className="px-0.5 text-subtle-foreground">/</span>
									) : null}
									<button
										type="button"
										aria-current={i === all.length - 1 ? "location" : undefined}
										onClick={() => browser.go(crumb.path)}
										className={cn(
											"cursor-pointer rounded px-1 py-0.5 hover:bg-accent",
											i === all.length - 1
												? "font-semibold text-foreground"
												: "text-muted-foreground"
										)}
									>
										{crumb.name}
									</button>
								</span>
							))
						: null}
				</nav>
				<label className="flex shrink-0 items-center gap-2 text-[12px] text-secondary-foreground">
					<Switch
						aria-label={translate("Show hidden")}
						checked={browser.hidden}
						onCheckedChange={browser.showHidden}
					/>
					{translate("Show hidden")}
				</label>
			</div>
			{naming ? <NewFolder browser={browser} onDone={() => setNaming(false)} /> : null}
			<div
				ref={list}
				role="listbox"
				tabIndex={0}
				aria-label={translate("Folders")}
				aria-busy={browser.loading}
				onKeyDown={keys}
				className="h-[280px] overflow-y-auto rounded-lg border border-input bg-subpanel p-1 outline-none focus-visible:border-primary"
			>
				{browser.failed ? (
					<p role="alert" className="px-2 py-1.5 text-[12px] text-status-error-text">
						{browser.failed}
					</p>
				) : listing === null ? (
					<p
						role="status"
						className="flex items-center gap-2 px-2 py-1.5 text-[12px] text-muted-foreground"
					>
						<Spinner size={11} />
						{translate("Loading…")}
					</p>
				) : entries.length === 0 ? (
					<p role="status" className="px-2 py-1.5 text-[12px] text-muted-foreground">
						{wants === "folder"
							? translate("No folders here.")
							: translate("Nothing here.")}
					</p>
				) : (
					entries.map((entry) => (
						<div
							key={entry.name}
							data-name={entry.name}
							role="option"
							aria-selected={entry.name === selected}
							onClick={() => browser.select(entry.name)}
							onDoubleClick={() =>
								entry.folder
									? browser.open(entry)
									: onChoose(joinPath(listing.path, entry.name))
							}
							className={cn(ROW, entry.name === selected && "bg-accent")}
						>
							{entry.folder ? (
								<Folder aria-hidden className={ICON} />
							) : (
								<File aria-hidden className={ICON} />
							)}
							<span className="min-w-0 flex-1 truncate">{entry.name}</span>
							<CheckoutTag entry={entry} />
							{entry.folder ? (
								<ChevronRight
									aria-hidden
									className="size-3.5 shrink-0 text-subtle-foreground"
								/>
							) : null}
						</div>
					))
				)}
				{listing?.truncated ? (
					<p className="px-2 py-1.5 text-[11.5px] text-muted-foreground">
						{translate(
							"This folder holds more than shows here. Type the path to reach the rest."
						)}
					</p>
				) : null}
			</div>
			<div className="flex items-center gap-2">
				<span
					title={chosen ?? undefined}
					className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
				>
					{chosen}
				</span>
				{wants === "folder" ? (
					<button
						type="button"
						className={SECONDARY}
						disabled={listing === null || !listing.exists || naming}
						onClick={() => setNaming(true)}
					>
						{translate("New folder")}
					</button>
				) : null}
				<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
				<button
					type="button"
					className={PRIMARY}
					disabled={chosen === null}
					onClick={choose}
				>
					{translate("Choose")}
				</button>
			</div>
		</div>
	);
};

/**
 * The folder browser for `device`, open while `open` is. It opens at
 * `start` and hands `onChoose` the folder, or the file when `wants` says
 * so, then closes.
 */
export const FolderBrowserDialog = ({
	device,
	start,
	wants = "folder",
	open,
	onOpenChange,
	onChoose,
}: {
	device: DeviceId;
	start: string;
	wants?: PathWants;
	open: boolean;
	onOpenChange: (open: boolean) => void;
	onChoose: (path: string) => void;
}) => (
	<Dialog.Root open={open} onOpenChange={onOpenChange}>
		<Dialog.Portal>
			<Dialog.Backdrop className="fixed inset-0 z-[130] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
			<Dialog.Popup
				// Keys stay in the browser, not in a form it opened from.
				onKeyDown={(e) => e.stopPropagation()}
				className="fixed top-1/2 left-1/2 z-[131] max-h-[calc(100vh-64px)] w-[min(560px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-xl border border-input bg-card p-5 shadow-popup outline-none"
			>
				<Browser
					device={device}
					start={start}
					wants={wants}
					onChoose={(path) => {
						onChoose(path);
						onOpenChange(false);
					}}
				/>
			</Dialog.Popup>
		</Dialog.Portal>
	</Dialog.Root>
);
