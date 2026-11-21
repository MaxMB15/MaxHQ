// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The parts the login pages share: the devices to tick, the pasted secret
// and its send, the key file picked or dropped and the offer to move it to
// the Trash, how each device took the login, and the list of what each
// device holds with Forget and any buttons a page adds.

import { type ReactNode, type RefObject, useRef, useState } from "react";

import { useServices } from "@/app/services";
import { CopyIcon } from "@/components/copy-field";
import { Checkbox } from "@/components/ui/checkbox";
import type { CredentialKind, DeviceId, Timestamp } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { cn } from "@/lib/utils";
import { moveToTrash, pickAttachments, useDropTarget } from "@/native";
import { forgetNote, type Outcome, outcomesOf, useTicks } from "@maxhq/shared/settings/login-parts";
import { formatAgo, sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";
import type { ViewState } from "@maxhq/shared/view/replica";

import { ConfirmButton, ErrorCopy, FIELD, Label, PRIMARY, SECONDARY, StatusNote } from "./controls";
import { fileName, type Held, type Target, usable, withoutWhitespace } from "./logins";
import { type Note, scrub } from "./result";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

export { type Outcome, useTicks };

/** The devices to send a login to, each with a tick. */
export const DeviceTicks = ({
	targets,
	ticked,
	onChange,
	disabled,
	label = translate("Devices that host workspaces"),
	empty = translate("No device has Incus yet, so there's nowhere to send it."),
	away = translate("Not connected"),
}: {
	targets: readonly Target[];
	ticked: ReadonlySet<DeviceId>;
	onChange: (ticked: ReadonlySet<DeviceId>) => void;
	disabled?: boolean;
	/** What the list is called. */
	label?: string;
	/** What shows when there's no device to send to. */
	empty?: string;
	/** What a device that isn't live says beside its name, if anything. */
	away?: string | null;
}) => {
	useI18n();
	return (
		<div role="group" aria-label={translate("Devices")} className="flex flex-col gap-1.5">
			<Label>{label}</Label>
			{targets.length === 0 ? (
				<span className="text-xs text-muted-foreground">{empty}</span>
			) : (
				targets.map((t) => (
					<label key={t.device} className="flex items-center gap-2.5 text-[12.5px]">
						<Checkbox
							aria-label={t.name}
							checked={ticked.has(t.device)}
							disabled={disabled || !usable(t)}
							onCheckedChange={(on) => {
								const next = new Set(ticked);
								if (on) next.add(t.device);
								else next.delete(t.device);
								onChange(next);
							}}
						/>
						<span
							className={cn("text-foreground", !usable(t) && "text-muted-foreground")}
						>
							{t.name}
						</span>
						{!t.live && away !== null ? (
							<span className="text-xs text-muted-foreground">{away}</span>
						) : t.live && t.blocked ? (
							<span className="text-xs text-muted-foreground">{t.blocked}</span>
						) : null}
					</label>
				))
			)}
		</div>
	);
};

/** One line for each device a login went to. */
export const Outcomes = ({
	view,
	outcomes,
	done,
}: {
	view: ViewState;
	outcomes: readonly Outcome[];
	/** What a device that took it shows, such as the repos it reaches. */
	done: (device: DeviceId) => ReactNode;
}) => {
	useI18n();
	return outcomes.length === 0 ? null : (
		<ul aria-label={translate("Results")} className="flex flex-col gap-1">
			{outcomes.map((o) => (
				<li key={o.device} className="flex flex-col gap-0.5 text-xs">
					<span
						className={cn(
							"font-[600]",
							o.failure ? "text-status-error-text" : "text-status-ready-text"
						)}
					>
						{o.failure ? (
							<>
								<span className="selectable">{`${deviceName(view, o.device)}: ${sentence(o.failure)}.`}</span>
								<ErrorCopy text={o.failure} />
							</>
						) : (
							translate("{{deviceName}} has it.", {
								deviceName: deviceName(view, o.device),
							})
						)}
					</span>
					{o.failure ? null : (
						<span className="selectable text-muted-foreground">{done(o.device)}</span>
					)}
				</li>
			))}
		</ul>
	);
};

/**
 * A key file, from the picker or a drop, since the backend reads only a
 * path offered that way. The page only ever holds the path.
 */
export const KeyFileField = ({
	label,
	path,
	onPath,
	onFailure,
	empty,
}: {
	label: string;
	path: string | null;
	onPath: (path: string) => void;
	/** Says the picker didn't open. */
	onFailure: (note: Note) => void;
	/** What the field says before a file is in it. */
	empty: string;
}) => {
	useI18n();
	const { ref: dropOn, over: dragging } = useDropTarget<HTMLDivElement>((paths) => {
		const [first] = paths;
		if (first) onPath(first);
	});
	const choose = async () => {
		try {
			const [picked] = await pickAttachments();
			if (picked) onPath(picked);
		} catch (error) {
			console.warn(translate("Couldn't open the file picker"), error);
			onFailure({
				text: translate("Couldn't open the file picker. Drop the file here instead."),
				error: true,
			});
		}
	};
	return (
		<div className="flex flex-col gap-1.5">
			<Label>{label}</Label>
			<div
				ref={dropOn}
				role="group"
				aria-label={label}
				className={cn(
					"flex items-center gap-2.5 rounded-[7px] border border-dashed border-border bg-subpanel px-3 py-2.5",
					dragging && "border-primary bg-primary/8"
				)}
			>
				<span className="flex min-w-0 flex-1 items-center gap-0.5">
					<span
						title={path ?? undefined}
						className={cn(
							"min-w-0 truncate text-xs",
							path ? "selectable font-mono text-foreground" : "text-muted-foreground"
						)}
					>
						{path ?? empty}
					</span>
					{path ? (
						<CopyIcon
							text={path}
							label={translate("Copy path")}
							className="size-[18px]"
						/>
					) : null}
				</span>
				<button type="button" className={SECONDARY} onClick={() => void choose()}>
					{path ? translate("Choose another") : translate("Choose file")}
				</button>
			</div>
			<span className="text-[11.5px] text-muted-foreground">
				{translate(
					"MaxHQ reads the file on the Mac's side and sends it on. This page never holds the key."
				)}
			</span>
		</div>
	);
};

/** Where the key file stands once the devices have it. */
export type Trash =
	| { readonly step: "offer"; readonly path: string }
	| { readonly step: "moving"; readonly path: string }
	| { readonly step: "moved"; readonly path: string }
	| { readonly step: "failed"; readonly path: string; readonly reason: string };

/**
 * The offer to move the key file to the Trash, and the move. `onMoved`
 * runs once the file is gone.
 */
export const useTrash = (
	onMoved: () => void
): [Trash | null, (trash: Trash | null) => void, (path: string) => Promise<void>] => {
	const [trash, setTrash] = useState<Trash | null>(null);
	const move = async (path: string) => {
		setTrash({ step: "moving", path });
		try {
			await moveToTrash(path);
			setTrash({ step: "moved", path });
			onMoved();
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			setTrash({ step: "failed", path, reason });
		}
	};
	return [trash, setTrash, move];
};

/** Offers to move the key file to the Trash, then says how that went. */
export const TrashOffer = ({ trash, onMove }: { trash: Trash; onMove: () => void }) => {
	useI18n();
	const name = fileName(trash.path);
	switch (trash.step) {
		case "moved":
			return (
				<p role="status" className="selectable text-xs text-muted-foreground">
					{translate("Moved {{name}} to the Trash. The Mac keeps no copy.", {
						name: name,
					})}
				</p>
			);
		case "failed":
			return (
				<p role="status" className="text-xs text-status-error-text">
					<span className="selectable">
						{translate(
							"Couldn't move {{name}} to the Trash: {{sentence}}. Delete it in Finder.",
							{ name: name, sentence: sentence(trash.reason) }
						)}
					</span>
					<ErrorCopy text={trash.reason} />
				</p>
			);
		default:
			return (
				<div
					role="group"
					aria-label={translate("Key file")}
					className="flex items-center gap-2.5 rounded-lg border border-input bg-subpanel px-3 py-2"
				>
					<span className="selectable min-w-0 flex-1 text-xs text-secondary-foreground">
						{translate("Every device has the key, so the Mac doesn't need {{name}}.", {
							name: name,
						})}
					</span>
					<button
						type="button"
						className={SECONDARY}
						disabled={trash.step === "moving"}
						onClick={onMove}
					>
						{translate("Move to Trash")}
					</button>
				</div>
			);
	}
};

/** A secret pasted into a field and sent to devices, with how it went. */
export type SecretSend = {
	/** The field. Only Send reads it, and it clears it as it does. */
	readonly field: RefObject<HTMLInputElement | null>;
	/** Whether the field holds something, never what. */
	readonly typed: boolean;
	readonly setTyped: (typed: boolean) => void;
	readonly busy: boolean;
	readonly outcomes: readonly Outcome[];
	readonly failure: string | null;
	/** Reads and clears the field, then sends what it held to `devices`. */
	readonly send: (devices: DeviceId[]) => Promise<void>;
};

/**
 * The state behind a pasted secret. `deliver` hands it to the source; no
 * state, store or log keeps it, and a reply that echoes it shows `<secret>`.
 */
export const useSecretSend = ({
	deliver,
	what,
	retry,
}: {
	deliver: (devices: DeviceId[], secret: string) => Promise<ActionResult[]>;
	/** What the secret is, for the console. */
	what: string;
	/** What shows when the send fails as a whole. */
	retry: string;
}): SecretSend => {
	const field = useRef<HTMLInputElement>(null);
	const [typed, setTyped] = useState(false);
	const [busy, setBusy] = useState(false);
	const [outcomes, setOutcomes] = useState<Outcome[]>([]);
	const [failure, setFailure] = useState<string | null>(null);

	const send = async (devices: DeviceId[]) => {
		const input = field.current;
		if (!input || busy) return;
		const secret = withoutWhitespace(input.value);
		input.value = "";
		setTyped(false);
		if (!secret) return;
		setBusy(true);
		setFailure(null);
		setOutcomes([]);
		try {
			setOutcomes(outcomesOf(devices, await deliver(devices, secret), secret));
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			console.warn(
				translate("Couldn't send the {{what}}:", { what: what }),
				scrub(reason, secret)
			);
			setFailure(retry);
		} finally {
			setBusy(false);
		}
	};

	return { field, typed, setTyped, busy, outcomes, failure, send };
};

/** The password field a secret is pasted into. */
export const SecretField = ({
	field,
	onTyped,
	label,
	placeholder,
}: {
	field: SecretSend["field"];
	onTyped: SecretSend["setTyped"];
	label: string;
	placeholder: string;
}) => {
	useI18n();
	return (
		<label className="flex flex-col gap-1.5">
			<Label>{label}</Label>
			<input
				ref={field}
				type="password"
				aria-label={label}
				placeholder={placeholder}
				autoComplete="off"
				spellCheck={false}
				autoCorrect="off"
				autoCapitalize="off"
				onInput={(e) => onTyped(e.currentTarget.value.trim() !== "")}
				className={cn(FIELD, "font-mono text-xs")}
			/>
		</label>
	);
};

/** How the send went, next to the Send button. */
export const SendFooter = ({
	view,
	secret,
	ready,
	done,
}: {
	view: ViewState;
	secret: SecretSend;
	/** Whether everything Send needs is there. */
	ready: boolean;
	done: (device: DeviceId) => ReactNode;
}) => {
	useI18n();
	return (
		<div className="flex items-center gap-3">
			<div className="min-w-0 flex-1">
				{secret.failure ? (
					<p role="status" className="text-xs text-status-error-text">
						<span className="selectable">{secret.failure}</span>
						<ErrorCopy text={secret.failure} />
					</p>
				) : (
					<Outcomes view={view} outcomes={secret.outcomes} done={done} />
				)}
			</div>
			<button
				type="submit"
				className={PRIMARY}
				disabled={!ready || !secret.typed || secret.busy}
			>
				{secret.busy ? translate("Sending") : translate("Send")}
			</button>
		</div>
	);
};

/** What the button that drops a login says, and its note while it runs. */
export type ForgetWords = { readonly label: string; readonly busy: string };

const FORGET: ForgetWords = {
	get label() {
		return translate("Forget");
	},
	get busy() {
		return translate("Forgetting");
	},
};

/** The button that drops a login a device holds, with a confirm step and how it went. */
export const Forget = ({
	device,
	credential,
	what,
	live,
	words: { label, busy },
}: {
	device: DeviceId;
	credential: string;
	what: string;
	live: boolean;
	words: ForgetWords;
}) => {
	useI18n();
	const { source } = useServices();
	const [note, setNote] = useState<Note | null>(null);
	const forget = async () => {
		setNote({ text: busy, error: false });
		try {
			setNote(forgetNote(await source.forgetCredential(device, credential)));
		} catch (error) {
			console.warn(translate("Couldn't forget the login"), error);
			setNote({ text: translate("Couldn't reach the device. Try again."), error: true });
		}
	};
	return (
		<>
			<StatusNote note={note} />
			<ConfirmButton
				label={label}
				question={`${label} ${what}?`}
				confirm={label}
				disabled={!live}
				onConfirm={() => void forget()}
			/>
		</>
	);
};

/** What the devices hold of one kind, each with Forget. */
export const HeldList = <T extends CredentialKind["type"]>({
	view,
	held,
	now,
	live,
	what,
	describe,
	actions,
	empty,
	forget = FORGET,
}: {
	view: ViewState;
	held: readonly Held<T>[];
	now: Timestamp;
	live: (device: DeviceId) => boolean;
	/** What Forget's question calls it, such as "the token". */
	what: string;
	/** The lines under the device's name. */
	describe: (credential: Held<T>) => ReactNode;
	/** Buttons before Forget, such as Check again. */
	actions?: (credential: Held<T>) => ReactNode;
	empty: string;
	/** What the button that drops it says, such as Sign out. */
	forget?: ForgetWords;
}) => {
	useI18n();
	return (
		<div className="flex flex-col">
			{held.length === 0 ? (
				<p className="px-4 py-3 text-xs text-muted-foreground">{empty}</p>
			) : (
				held.map((c) => (
					<div
						key={c.id}
						role="group"
						aria-label={translate("On {{deviceName}}", {
							deviceName: deviceName(view, c.device),
						})}
						className="flex items-start gap-4 border-t border-sidebar-border px-4 py-3 first:border-t-0"
					>
						<div className="flex min-w-0 flex-1 flex-col gap-0.5">
							<span className="selectable text-[12.5px] text-foreground">
								{deviceName(view, c.device)}
							</span>
							<span className="selectable text-xs text-muted-foreground">
								{describe(c)}
							</span>
							<span className="text-xs text-muted-foreground">
								{translate("Stored {{ago}}", { ago: formatAgo(c.stored_at, now) })}
							</span>
							{c.problem ? (
								<span className="text-xs text-status-error-text">
									<span className="selectable">{`${sentence(c.problem)}.`}</span>
									<ErrorCopy text={c.problem} />
								</span>
							) : null}
						</div>
						<div className="flex shrink-0 items-center gap-2.5">
							{actions?.(c)}
							<Forget
								device={c.device}
								credential={c.id}
								what={translate("{{what}} on {{deviceName}}", {
									what: what,
									deviceName: deviceName(view, c.device),
								})}
								live={live(c.device)}
								words={forget}
							/>
						</div>
					</div>
				))
			)}
		</div>
	);
};
