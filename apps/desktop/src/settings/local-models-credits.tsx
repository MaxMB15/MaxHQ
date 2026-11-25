// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A cloud model server's credits on its row in the Local models settings:
// the figures in one line, dimmed after a failed read with why in the
// tooltip, and, for an OpenRouter server, the admin key that reads the
// account's balance. The form that adds one says what such a key can do and
// links to where OpenRouter makes one. The key goes from a password field
// straight into the action, which clears the field; no state, log or note
// keeps it. Remove takes one click.

import { useRef, useState } from "react";

import { useView } from "@/app/services";
import { cn } from "@/lib/utils";
import type { DeviceId, ModelServer } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	ADMIN_KEYS_URL,
	adminKeyWarning,
	creditsView,
	offersAdminKey,
	removeAdminKeyAction,
	setAdminKeyAction,
} from "@maxhq/shared/local-models/credits";
import { useNow } from "@maxhq/shared/lib/clock";

import { PRIMARY, SECONDARY, StatusNote } from "./controls";
import { useModelAction } from "./local-models-catalog";
import { SecretField } from "./login-parts";

const LINK = "text-primary hover:underline disabled:opacity-60 disabled:no-underline";

/** A server's credits in one line, or nothing without them. */
export const CreditsLine = ({ server }: { server: Pick<ModelServer, "credits"> }) => {
	useI18n();
	const now = useNow(10_000);
	if (!server.credits) return null;
	const shown = creditsView(server.credits, now);
	return (
		<span
			title={shown.title}
			data-stale={shown.stale || undefined}
			className={cn("text-muted-foreground tabular-nums", shown.stale && "opacity-55")}
		>
			{shown.line}
		</span>
	);
};

/**
 * The admin key of an OpenRouter server: "Admin key added" with Remove once
 * `maxd` holds one, and otherwise the button that opens the form adding it.
 */
export const AdminKey = ({
	device,
	server,
	disabled,
}: {
	device: DeviceId;
	server: ModelServer;
	disabled: boolean;
}) => {
	useI18n();
	const view = useView((s) => s.view);
	const { busy, note, send } = useModelAction(device);
	const field = useRef<HTMLInputElement>(null);
	const [open, setOpen] = useState(false);
	const [typed, setTyped] = useState(false);
	if (!offersAdminKey(view, device, server)) return null;
	if (server.admin_key) {
		return (
			<span className="flex items-center gap-2 text-muted-foreground">
				{translate("Admin key added")}
				<button
					type="button"
					className={LINK}
					disabled={disabled || busy}
					onClick={() => void send(removeAdminKeyAction(server.id))}
				>
					{translate("Remove")}
				</button>
				<StatusNote note={note} />
			</span>
		);
	}
	if (!open) {
		return (
			<span className="flex items-center gap-2">
				<button
					type="button"
					className={cn(LINK, "self-start")}
					disabled={disabled}
					onClick={() => setOpen(true)}
				>
					{translate("Add admin key for your balance")}
				</button>
				<StatusNote note={note} />
			</span>
		);
	}
	/** Sends the key typed, which it reads and clears, and closes the form once it's kept. */
	const add = async () => {
		const input = field.current;
		const key = input?.value ?? "";
		if (input) input.value = "";
		setTyped(false);
		if (await send(setAdminKeyAction(server.id, key))) setOpen(false);
	};
	return (
		<div
			role="group"
			aria-label={translate("Admin key")}
			className="mt-1 flex flex-col gap-2.5 rounded-[7px] border border-border bg-subpanel p-3"
		>
			<p className="text-secondary-foreground">
				{adminKeyWarning()}{" "}
				<a href={ADMIN_KEYS_URL} target="_blank" rel="noreferrer" className={LINK}>
					{translate("Make one on OpenRouter")}
				</a>
			</p>
			<SecretField
				field={field}
				onTyped={setTyped}
				label={translate("Admin key")}
				placeholder={translate("Kept by maxd, never shown again")}
			/>
			<div className="flex items-center justify-end gap-2.5">
				<StatusNote note={note} className="mr-auto" />
				<button
					type="button"
					className={SECONDARY}
					disabled={busy}
					onClick={() => setOpen(false)}
				>
					{translate("Cancel")}
				</button>
				<button
					type="button"
					className={PRIMARY}
					disabled={disabled || busy || !typed}
					onClick={() => void add()}
				>
					{busy ? translate("Checking") : translate("Add")}
				</button>
			</div>
		</div>
	);
};
