// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The licences a device's installs wait on, each in full with Accept.
// Accept sends the text's SHA-256 as read, so a licence that changed since
// asks again rather than being accepted unseen.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices } from "@/app/services";
import { cn } from "@/lib/utils";
import { PRIMARY, SECONDARY, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import { BACKDROP, POPUP, Title } from "@/workspaces/new-workspace";
import type { DeviceId } from "@maxhq/shared/bindings";
import { type LicenceAsk, pendingLicences, toolName } from "@maxhq/shared/hosts/tools";
import type { ViewState } from "@maxhq/shared/view/replica";

const Licence = ({ ask }: { ask: LicenceAsk }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const { device, licence, tool } = ask;
	const accept = async () => {
		setBusy(true);
		setNote({ text: t("Sending"), error: false });
		try {
			const failure = failureOf(
				await source.acceptLicence(device, licence.id, licence.text_sha256)
			);
			setNote(failure ? { text: failure, error: true } : null);
		} catch (error) {
			console.warn("Couldn't accept the licence", error);
			setNote({ text: t("Couldn't reach it. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return (
		<section aria-label={licence.id} className="flex flex-col gap-1.5">
			<h3 className="text-[12px] font-[600] text-foreground">
				{licence.id}{" "}
				<span className="font-normal text-muted-foreground">
					{t("for {{value2}}", { value2: toolName(tool.kind) })}
				</span>
			</h3>
			<pre
				tabIndex={0}
				aria-label={t("{{id}} text", { id: licence.id })}
				className="max-h-[38vh] overflow-y-auto rounded-[7px] border border-border bg-subpanel px-2.5 py-2 font-mono text-[11.5px] whitespace-pre-wrap text-foreground select-text"
			>
				{licence.text}
			</pre>
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<button
					type="button"
					className={PRIMARY}
					aria-label={t("Accept {{id}}", { id: licence.id })}
					disabled={busy}
					onClick={() => void accept()}
				>
					{t("Accept")}
				</button>
			</div>
		</section>
	);
};

/** The licences `device`'s installs wait on, while `open`. */
export const LicenceDialog = ({
	device,
	view,
	open,
	onClose,
}: {
	device: DeviceId;
	view: ViewState;
	open: boolean;
	onClose: () => void;
}) => {
	const { t } = useI18n();

	const asks = pendingLicences(view).filter((ask) => ask.device === device);
	return (
		<Dialog.Root open={open} onOpenChange={(next) => (next ? undefined : onClose())}>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={cn(POPUP, "w-[min(640px,calc(100vw-32px))]")}>
					<div className="flex flex-col gap-4">
						<Title>{t("Licences to accept")}</Title>
						<p className="text-[12.5px] text-secondary-foreground">
							{t(
								"The install goes on once you accept each one. MaxHQ keeps which text you accepted, and asks again if it changes."
							)}
						</p>
						{asks.length === 0 ? (
							<p className="text-[12.5px] text-muted-foreground">
								{t("Nothing waits on a licence.")}
							</p>
						) : (
							asks.map((ask) => (
								<Licence
									key={`${ask.licence.id}-${ask.tool.place.type === "host" ? "host" : ask.tool.place.workspace}`}
									ask={ask}
								/>
							))
						)}
						<div className="flex justify-end">
							<Dialog.Close className={SECONDARY}>{t("Close")}</Dialog.Close>
						</div>
					</div>
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
