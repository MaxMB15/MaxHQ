// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a device's webhooks are reached: only on the device, over Tailscale
// Funnel, or at an address of your own such as a tunnel. Funnel asks first,
// since maxd then changes the device's Funnel setup. Each hook keeps its
// token, so only the start of its URL changes.

import { useI18n } from "@maxhq/shared/i18n";

import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";

import { useServices } from "@/app/services";
import { Picker } from "@/new-agent/picker";
import { FIELD, PRIMARY, Row, SECONDARY, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import { cn } from "@/lib/utils";
import type { DeviceTasks, HookAddress } from "@maxhq/shared/bindings";
import { ADDRESS_LABEL, addressOf } from "@maxhq/shared/tasks/format";

import { AddressNote } from "./hook-fields";

const KINDS = Object.entries(ADDRESS_LABEL).map(([value, label]) => ({ value, label }));

/** Asks before maxd adds its paths to the device's Funnel. */
const FunnelConfirm = ({
	device,
	open,
	onAnswer,
}: {
	device: string;
	open: boolean;
	onAnswer: (yes: boolean) => void;
}) => {
	const { t } = useI18n();
	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next) onAnswer(false);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup className="fixed top-[16vh] left-1/2 z-[121] flex w-[min(440px,calc(100vw-32px))] -translate-x-1/2 flex-col gap-4 rounded-xl border border-input bg-card p-5 shadow-popup outline-none">
					<div className="flex flex-col gap-1">
						<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
							{t("Use Tailscale Funnel on {{device}}?", { device: device })}
						</Dialog.Title>
						<Dialog.Description className="text-[12.5px] text-muted-foreground">
							{t(
								"maxd runs tailscale funnel on {{device}} to add one path under /maxhq for each webhook that's on, and removes each path when its webhook goes. It leaves the rest of the device's Funnel and Serve setup alone. Anyone with a webhook's URL can reach it from the internet.",
								{ device: device }
							)}
						</Dialog.Description>
					</div>
					<div className="flex justify-end gap-2">
						<Dialog.Close className={SECONDARY}>{t("Cancel")}</Dialog.Close>
						<button type="button" className={PRIMARY} onClick={() => onAnswer(true)}>
							{t("Use Funnel")}
						</button>
					</div>
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};

/** A device's webhook address, with the picker that changes it. */
export const AddressRow = ({ reach, name }: { reach: DeviceTasks; name: string }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { funnel } = reach;
	const address = addressOf(reach);
	// The own address being typed, before it's sent.
	const [own, setOwn] = useState<string | null>(null);
	const [asking, setAsking] = useState(false);
	const [note, setNote] = useState<Note | null>(null);

	const send = async (next: HookAddress) => {
		setNote(null);
		try {
			const failure = failureOf(await source.setHookAddress(reach.device, next));
			if (failure) return setNote({ text: failure, error: true });
			setOwn(null);
		} catch (error) {
			console.warn("Couldn't change the webhook address", error);
			setNote({ text: t("Couldn't change the address. Try again."), error: true });
		}
	};

	const pick = (kind: string) => {
		setNote(null);
		setOwn(null);
		if (kind === "funnel") setAsking(true);
		else if (kind === "own") setOwn(address.kind === "own" ? address.base : "");
		else if (address.kind !== "local") void send({ kind: "local" });
	};

	return (
		<>
			<Row
				title={t("Webhook address")}
				sub={
					own === null ? (
						<AddressNote reach={reach} device={name} />
					) : (
						<span className="block">
							{t(
								"Paste the address of a tunnel or proxy that points at{{value2}}{{value4}}. It needs https, or http on this device. maxd can't check that it works.",
								{
									value2: " ",
									value4: reach.port
										? `http://127.0.0.1:${reach.port}`
										: t("this device's hooks port"),
								}
							)}
						</span>
					)
				}
			>
				<StatusNote note={note} />
				{address.kind === "funnel" && funnel.state === "needs_setup" && own === null ? (
					<button
						type="button"
						className={SECONDARY}
						onClick={() => void send({ kind: "funnel" })}
					>
						{t("Try again")}
					</button>
				) : null}
				<Picker
					label={t("Reached")}
					value={own === null ? address.kind : "own"}
					options={KINDS}
					onChange={pick}
				/>
			</Row>
			{own === null ? null : (
				<form
					aria-label={t("Your own address")}
					className="flex items-center gap-2 px-4 pb-3"
					onSubmit={(e) => {
						e.preventDefault();
						void send({ kind: "own", base: own.trim() });
					}}
				>
					<input
						aria-label={t("Address")}
						value={own}
						onChange={(e) => setOwn(e.target.value)}
						placeholder="https://hooks.example.com"
						spellCheck={false}
						autoComplete="off"
						className={cn(FIELD, "min-w-0 flex-1 font-mono text-xs")}
					/>
					<button type="button" className={SECONDARY} onClick={() => setOwn(null)}>
						{t("Cancel")}
					</button>
					<button type="submit" className={PRIMARY} disabled={own.trim() === ""}>
						{t("Use address")}
					</button>
				</form>
			)}
			<FunnelConfirm
				device={name}
				open={asking}
				onAnswer={(yes) => {
					setAsking(false);
					if (yes) void send({ kind: "funnel" });
				}}
			/>
		</>
	);
};
