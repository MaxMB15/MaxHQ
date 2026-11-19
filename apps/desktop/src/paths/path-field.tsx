// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The one field for a path on a device, wherever the app asks for one. You
// type, and it suggests what's in the folder you're typing in, from that
// device. Browse… opens the system picker for this Mac and MaxHQ's own
// folder browser for any other device. A folder dropped from Finder goes
// in on a field for this Mac, and on another device's field the field says
// it's on this Mac and offers that device's checkout of the same repo. What
// it does lives in @maxhq/shared/folders; this draws it.

import { File, Folder } from "lucide-react";
import { type KeyboardEvent, useEffect, useId, useState } from "react";

import { useServices, useView } from "@/app/services";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useDropTarget } from "@/native/drop";
import { describePaths, type PathInfo, pickPath } from "@/native/files";
import { inApp } from "@/native/platform";
import { SECONDARY } from "@/settings/controls";
import type { DeviceId } from "@maxhq/shared/bindings";
import { startOf } from "@maxhq/shared/folders/browser";
import {
	checkoutProject,
	type Dropped,
	dropHint,
	dropOutcome,
	sameRepoFolder,
} from "@maxhq/shared/folders/drop";
import { usePathField } from "@maxhq/shared/folders/field";
import { expandHome, parentOf, pickText, type PathWants } from "@maxhq/shared/folders/path";
import { browseWay, supportsFolders } from "@maxhq/shared/folders/select";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { isThisMac } from "@maxhq/shared/settings/app-keys";
import { macDevice } from "@maxhq/shared/settings/logins";
import { deviceName } from "@maxhq/shared/view/select";

import { CheckoutTag, FolderBrowserDialog } from "./folder-browser";

const ROW =
	"flex h-7 w-full cursor-default items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px] text-foreground";

const ICON = "size-3.5 shrink-0 text-muted-foreground";

/** The field while a drop it takes is over it. */
const DROP_OK = "border-dashed border-primary bg-primary/10";

/** The field while a drop from this Mac is over another device's field, or after one. */
const DROP_BAD = "border-dashed border-status-error bg-status-error/5";

const SMALL = cn(SECONDARY, "h-[26px] px-2.5 text-[12px]");

/** A path with no slash at its end, as describePaths may give a folder. */
const trimmed = (path: string) => path.replace(/\/+$/u, "") || "/";

/** A drop, and the device's field it landed on. */
type Landed = { readonly device: DeviceId; readonly dropped: Dropped };

/** The other device's checkout of a dropped folder's repo. */
type Twin = { readonly dropped: Dropped; readonly path: string };

/**
 * A path on `device`, a folder or a file as `wants` says. `title` names
 * the system picker by what the path is for, such as "Choose the agent's
 * folder". Text that isn't a path, such as a name on PATH, stays as typed.
 */
export const PathField = ({
	device,
	value,
	onChange,
	label,
	title,
	wants = "folder",
	placeholder,
	disabled = false,
	className,
}: {
	device: DeviceId | null;
	value: string;
	onChange: (value: string) => void;
	/** The field's name above it. */
	label: string;
	title: string;
	wants?: PathWants;
	placeholder?: string;
	disabled?: boolean;
	/** The text box's look. */
	className: string;
}) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const id = useId();
	const list = `${id}-suggestions`;
	const lists = device !== null && supportsFolders(view, device);
	const field = usePathField({ calls: services.source, device, lists, value, onChange, wants });
	const [focused, setFocused] = useState(false);
	const [closed, setClosed] = useState(false);
	/** The suggestion you moved to with the arrows or the pointer. */
	const [active, setActive] = useState<number | null>(null);
	const [browsing, setBrowsing] = useState(false);
	const [landed, setLanded] = useState<Landed | null>(null);
	const [twin, setTwin] = useState<Twin | null>(null);
	/** The device's field a drop couldn't be read on. */
	const [unread, setUnread] = useState<DeviceId | null>(null);

	const here = device !== null && isThisMac(view, device);
	const way = device === null ? null : browseWay(view, device, inApp());
	const name = device === null ? "" : deviceName(view, device);
	const dropped = landed !== null && landed.device === device ? landed.dropped : null;
	const outcome = dropped === null ? null : dropOutcome(dropped, wants, here, field.home);
	const twinPath = twin !== null && twin.dropped === dropped ? twin.path : null;

	const { typed, entries } = field;
	const menu = focused && !closed && !disabled && typed !== null && entries.length > 0;
	// Until you move to one, the suggestion marked is the first whose name
	// starts with exactly what you typed, and none when no name does.
	const starts = typed?.name ? entries.findIndex((e) => e.name.startsWith(typed.name)) : -1;
	const at = active !== null && active < entries.length ? active : starts < 0 ? null : starts;
	const optionId = (index: number) => `${list}-${index}`;

	useEffect(() => {
		if (menu && at !== null)
			document.getElementById(`${list}-${at}`)?.scrollIntoView?.({ block: "nearest" });
	}, [menu, list, at]);

	/** Puts a path you picked, chose or dropped in the field. */
	const put = (path: string, close = true) => {
		field.change(path);
		setActive(null);
		setClosed(close);
		setLanded(null);
		setUnread(null);
	};

	const keys = (event: KeyboardEvent<HTMLInputElement>) => {
		if (!menu || typed === null) return;
		const last = entries.length - 1;
		switch (event.key) {
			case "ArrowDown":
				setActive(at === null || at === last ? 0 : at + 1);
				break;
			case "ArrowUp":
				setActive(at === null || at === 0 ? last : at - 1);
				break;
			case "Tab": {
				const entry = entries[at ?? 0];
				// A Tab with nothing left to complete moves on, as Tab does.
				const next = entry ? pickText(typed, entry, true) : field.text;
				if (event.shiftKey || next === field.text) return;
				put(next, false);
				break;
			}
			case "Enter": {
				// ↩ picks the suggestion marked. With none, or one the field
				// already holds, the text stays and the form gets the ↩.
				const entry = at === null ? undefined : entries[at];
				const next = entry ? pickText(typed, entry, false) : field.text;
				if (next === field.text) {
					setClosed(true);
					return;
				}
				put(next);
				break;
			}
			case "Escape":
				// Closes the list, not the dialog the field is in.
				event.stopPropagation();
				setClosed(true);
				break;
			default:
				return;
		}
		event.preventDefault();
	};

	/** Where the system picker opens: the folder the field holds, when it's a full path. */
	const nativeStart = (): string | null => {
		const at = expandHome(startOf(field.text), field.home);
		const folder = wants === "file" ? parentOf(at) : at;
		return folder?.startsWith("/") ? folder : null;
	};

	const browse = () => {
		if (disabled || way === null) return;
		if (way.way === "browser") return setBrowsing(true);
		if (way.way === "native")
			void pickPath(title, nativeStart(), wants === "file").then((path) => {
				if (path !== null) put(path);
			});
	};

	const drop = async (paths: string[]) => {
		const first = paths[0];
		if (first === undefined || device === null || disabled) return;
		let info: PathInfo | undefined;
		try {
			[info] = await describePaths([first]);
		} catch {
			// The field keeps what it held and says the drop didn't take.
			setLanded(null);
			setTwin(null);
			setUnread(device);
			return;
		}
		setUnread(null);
		const dropped: Dropped = {
			path: trimmed(info?.path ?? first),
			folder: info?.kind === "folder",
		};
		const out = dropOutcome(dropped, wants, here, field.home);
		if (out.kind === "use") put(out.path);
		setLanded({ device, dropped });
		setTwin(null);
		const mac = macDevice(view);
		if (out.kind !== "elsewhere" || mac === null) return;
		const project = await checkoutProject(services.source, mac, out.folder);
		const path = project === null ? null : sameRepoFolder(view, project, device);
		if (path !== null) setTwin({ dropped, path });
	};
	const { ref: dropOn, over: dragging } = useDropTarget<HTMLDivElement>(
		(paths) => void drop(paths)
	);
	const over = dragging && !disabled && device !== null;
	const bad = (over && !here) || outcome?.kind === "elsewhere";

	const browseButton = (text: string, look: string) => {
		const off = disabled || way === null || way.way === "none";
		return (
			<Tooltip content={way?.way === "none" ? way.why : null}>
				<button
					type="button"
					aria-disabled={off || undefined}
					onClick={browse}
					className={cn(look, off && "cursor-not-allowed opacity-50")}
				>
					{text}
				</button>
			</Tooltip>
		);
	};

	/** What the line under the field says, and how. */
	const line = ((): { text: string; tone: "muted" | "error" } | null => {
		if (over) return { text: dropHint(wants, here), tone: here ? "muted" : "error" };
		if (unread !== null && unread === device)
			return { text: translate("Couldn't read what you dropped. Try again."), tone: "error" };
		if (outcome?.kind === "use")
			return outcome.note ? { text: outcome.note, tone: "muted" } : null;
		if (outcome?.kind === "refuse") return { text: outcome.why, tone: "error" };
		if (outcome?.kind !== "elsewhere") return null;
		const has = twinPath
			? ` ${translate("{{device}} has a checkout of the same repo at {{path}}.", {
					device: name,
					path: twinPath,
				})}`
			: "";
		return { text: outcome.why + has, tone: "muted" };
	})();

	return (
		<div className="flex min-w-0 flex-col gap-1.5">
			<label htmlFor={id} className="text-[11.5px] font-[550] text-secondary-foreground">
				{label}
			</label>
			<div ref={dropOn} className="flex min-w-0 gap-2">
				<div className="relative min-w-0 flex-1">
					<input
						id={id}
						value={field.text}
						onChange={(e) => {
							field.change(e.target.value);
							setActive(null);
							setClosed(false);
							setLanded(null);
							setUnread(null);
						}}
						onKeyDown={keys}
						onFocus={() => setFocused(true)}
						onBlur={() => setFocused(false)}
						placeholder={placeholder}
						disabled={disabled}
						spellCheck={false}
						autoComplete="off"
						autoCorrect="off"
						autoCapitalize="off"
						aria-autocomplete="list"
						aria-controls={menu ? list : undefined}
						aria-activedescendant={menu && at !== null ? optionId(at) : undefined}
						className={cn(
							className,
							"font-mono text-xs",
							over && here && DROP_OK,
							bad && DROP_BAD
						)}
					/>
					{menu ? (
						<div
							id={list}
							role="listbox"
							aria-label={translate("Suggestions")}
							className="absolute top-full left-0 z-20 mt-1 max-h-64 w-max max-w-[28rem] min-w-full overflow-y-auto rounded-lg border border-input bg-popover p-1 shadow-lg"
							onMouseDown={(e) => e.preventDefault()}
						>
							{entries.map((entry, index) => (
								<div
									key={entry.name}
									id={optionId(index)}
									role="option"
									aria-selected={index === at}
									className={cn(ROW, index === at && "bg-accent")}
									onMouseMove={() => {
										if (index !== at) setActive(index);
									}}
									onClick={() => put(pickText(typed, entry, false))}
								>
									{entry.folder ? (
										<Folder aria-hidden className={ICON} />
									) : (
										<File aria-hidden className={ICON} />
									)}
									<span className="min-w-0 flex-1 truncate font-mono text-xs">
										{pickText(typed, entry, false)}
									</span>
									<CheckoutTag entry={entry} />
								</div>
							))}
							<div className="px-2 pt-1 pb-0.5 text-[11px] text-muted-foreground">
								{translate("Tab completes · ↩ picks · on {{device}}", {
									device: name,
								})}
							</div>
						</div>
					) : null}
				</div>
				{browseButton(translate("Browse…"), cn(SECONDARY, "h-8 shrink-0"))}
			</div>
			{line ? (
				<p
					role={line.tone === "error" ? "alert" : "status"}
					className={cn(
						"text-[11.5px] leading-snug",
						line.tone === "error" ? "text-status-error-text" : "text-muted-foreground"
					)}
				>
					{line.text}
				</p>
			) : null}
			{outcome?.kind === "elsewhere" && !over ? (
				<div className="flex flex-wrap items-center gap-2">
					{twinPath ? (
						<button type="button" className={SMALL} onClick={() => put(twinPath)}>
							{translate("Use that")}
						</button>
					) : null}
					{browseButton(translate("Browse {{device}}…", { device: name }), SMALL)}
				</div>
			) : null}
			{browsing && device !== null ? (
				<FolderBrowserDialog
					device={device}
					start={startOf(field.text)}
					wants={wants}
					open
					onOpenChange={(open) => {
						setBrowsing(open);
						// A folder made there shows in the suggestions after.
						if (!open) field.reload();
					}}
					onChoose={(path) => put(path)}
				/>
			) : null}
		</div>
	);
};
