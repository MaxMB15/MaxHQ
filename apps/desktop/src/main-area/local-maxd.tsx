// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// This Mac's maxd from the app. While MaxHQ can't reach it, a notice says
// so wherever agents and notes would show, with Start maxd and the way to
// this Mac's page. That page starts it and uninstalls it, with or without
// MaxHQ's data on this Mac. A new Mac gets going without Terminal.

import { t, useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices, useUi, useView } from "@/app/services";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { DANGER, PRIMARY, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { DeviceId, DeviceLink } from "@maxhq/shared/bindings";
import { sentence } from "@maxhq/shared/status/format";
import { all } from "@maxhq/shared/view/replica";

/** This Mac's link, when the app has one. */
export const useLocalLink = (): DeviceLink | undefined =>
	useView((s) => all(s.view, "device_link").find((l) => l.route.type === "local"));

/** Starts this Mac's maxd: whether it's on it, what it last said, and the call. */
export const useStartMaxd = () => {
	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const start = async () => {
		setBusy(true);
		setNote({ text: t("Starting"), error: false });
		try {
			const failure = failureOf(await source.startLocalMaxd());
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't start maxd", error);
			setNote({ text: t("Couldn't start maxd. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return { busy, note, start };
};

/** The button that starts this Mac's maxd. */
export const StartMaxdButton = ({
	busy,
	start,
	className = PRIMARY,
}: {
	busy: boolean;
	start: () => Promise<void>;
	className?: string;
}) => {
	const { t } = useI18n();
	return (
		<button type="button" className={className} disabled={busy} onClick={() => void start()}>
			{t("Start maxd")}
		</button>
	);
};

/**
 * Says MaxHQ can't reach this Mac's maxd, with Start maxd and a button to
 * this Mac's page. Nothing while it's connected or still trying. With
 * `device`, only when that's this Mac or no device yet.
 */
export const LocalMaxdNotice = ({
	device,
	onOpen,
	className,
}: {
	device?: DeviceId | null;
	/** Runs once the button to this Mac's page took you there. */
	onOpen?: () => void;
	className?: string;
}) => {
	const { t } = useI18n();
	const link = useLocalLink();
	const selectLink = useUi((s) => s.selectLink);
	const { busy, note, start } = useStartMaxd();
	if (link?.state.state !== "unreachable") return null;
	if (device && device !== link.device) return null;
	const open = () => {
		selectLink(link.id);
		onOpen?.();
	};
	return (
		<section
			aria-label={t("maxd on this Mac")}
			className={cn(
				"flex flex-col gap-2 rounded-[10px] border border-status-error/35 bg-status-error/6 px-3.5 py-3",
				className
			)}
		>
			<div className="flex flex-col gap-0.5">
				<h3 className="text-[12.5px] font-[650] text-foreground-strong">
					{t("Can't reach maxd on this Mac")}
				</h3>
				<p className="selectable text-xs text-muted-foreground">
					{t("{{reason}}. Start it to run agents on this Mac.", {
						reason: sentence(link.state.reason),
					})}
				</p>
			</div>
			<div className="flex flex-wrap items-center gap-2">
				<StartMaxdButton busy={busy} start={start} />
				<button type="button" className={SECONDARY} onClick={open}>
					{t("Open {{name}}", { name: link.name })}
				</button>
				<StatusNote note={note} />
			</div>
		</section>
	);
};

/** Starts this Mac's maxd, shown while its link isn't connected. */
const StartMaxd = () => {
	const { t } = useI18n();
	const { busy, note, start } = useStartMaxd();
	return (
		<Row
			title={t("Start maxd")}
			sub={t(
				"maxd runs your agents on this Mac. This starts it now and each time you log in."
			)}
		>
			<StatusNote note={note} />
			<StartMaxdButton busy={busy} start={start} />
		</Row>
	);
};

const done = (): Note => ({
	text: t("maxd is uninstalled. Start maxd puts it back."),
	error: false,
});

/** Uninstalls this Mac's maxd, once the dialog says what that means. */
const UninstallMaxd = () => {
	const { t } = useI18n();
	const { source } = useServices();
	const [open, setOpen] = useState(false);
	const [wipe, setWipe] = useState(false);
	const [busy, setBusy] = useState(false);
	// Once the app's data is gone the app quits, so nothing more happens here.
	const [quitting, setQuitting] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const [result, setResult] = useState<Note | null>(null);
	const stuck = busy || quitting;
	const show = () => {
		setWipe(false);
		setNote(null);
		setResult(null);
		setOpen(true);
	};
	const uninstall = async () => {
		const deleting = wipe;
		setBusy(true);
		setNote({ text: t("Uninstalling"), error: false });
		try {
			const failure = failureOf(await source.uninstallLocalMaxd(deleting));
			if (failure) {
				setNote({ text: failure, error: true });
			} else if (deleting) {
				setQuitting(true);
				setNote({ text: t("Done. MaxHQ is quitting."), error: false });
			} else {
				setOpen(false);
				setNote(null);
				setResult(done());
			}
		} catch (error) {
			console.warn("Couldn't uninstall maxd", error);
			setNote({ text: t("Couldn't uninstall maxd. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return (
		<Row
			title={t("Uninstall maxd")}
			sub={t("Stops maxd on this Mac and takes it out of your login items.")}
		>
			<StatusNote note={result} />
			<button type="button" className={SECONDARY} onClick={show}>
				{t("Uninstall maxd")}
			</button>
			<Dialog.Root
				open={open}
				onOpenChange={(next) => {
					if (!stuck) setOpen(next);
				}}
			>
				<Dialog.Portal>
					<Dialog.Backdrop className={BACKDROP} />
					<Dialog.Popup className={cn(POPUP, "w-[min(480px,calc(100vw-32px))]")}>
						<div className="flex flex-col gap-4">
							<Title>{t("Uninstall maxd on this Mac?")}</Title>
							<div className="flex flex-col gap-1.5 text-[12.5px] text-secondary-foreground">
								<p>
									{t("maxd runs your agents on this Mac. Once you uninstall it:")}
								</p>
								<ul className="flex list-disc flex-col gap-1 pl-4">
									<li>{t("Every agent on this Mac stops.")}</li>
									<li>
										{t(
											"Your phone and your other devices can't reach this Mac."
										)}
									</li>
									<li>{t("maxd no longer starts when you log in.")}</li>
								</ul>
							</div>
							<div className="flex flex-col gap-1">
								<label className="flex items-center gap-2 text-[12.5px] text-foreground">
									<Checkbox
										aria-label={t("Also delete MaxHQ's data on this Mac")}
										checked={wipe}
										onCheckedChange={setWipe}
										disabled={stuck}
									/>
									{t("Also delete MaxHQ's data on this Mac")}
								</label>
								<p className="pl-6 text-xs text-muted-foreground">
									{t(
										"That's your agent history, the notes kept on this Mac, your settings and your keys. MaxHQ quits once it's done."
									)}
								</p>
							</div>
							<p className="text-xs text-muted-foreground">
								{t(
									"To remove MaxHQ, uninstall maxd here, then move MaxHQ to the Trash. If you trash MaxHQ first, maxd removes itself within about five minutes."
								)}
							</p>
							<div className="flex items-center justify-end gap-2.5">
								<StatusNote note={note} className="mr-auto" />
								<Dialog.Close className={SECONDARY} disabled={stuck}>
									{t("Cancel")}
								</Dialog.Close>
								<button
									type="button"
									className={DANGER}
									disabled={stuck}
									onClick={() => void uninstall()}
								>
									{wipe ? t("Uninstall and delete") : t("Uninstall maxd")}
								</button>
							</div>
						</div>
					</Dialog.Popup>
				</Dialog.Portal>
			</Dialog.Root>
		</Row>
	);
};

/** This Mac's maxd on its page: Start maxd while it isn't connected, and Uninstall. */
export const LocalMaxd = ({ link }: { link: DeviceLink }) => {
	const { t } = useI18n();
	return (
		<Section
			title="maxd"
			label={t("maxd on {{name}}", { name: link.name })}
			hint={t("Runs your agents on this Mac")}
		>
			{link.state.state === "connected" ? null : <StartMaxd />}
			<UninstallMaxd />
		</Section>
	);
};
