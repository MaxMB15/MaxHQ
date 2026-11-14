// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Sign in again: the window's dialog that runs a Claude, Codex or Cursor
// login's own CLI login on the device that holds it. It begins as it opens,
// shows the page to sign in on, takes the code Claude's page shows, and
// asks the device every 2 s until the CLI stored the login. Closing it
// while it waits stops the CLI. The code you paste sits only in the field
// until Sign in reads it, clears it and hands it to the source.

import { Dialog } from "@base-ui/react/dialog";
import { useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { Spinner } from "@/agent-view/icons";
import { type Services, useServices, useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import { CopyIcon } from "@/components/copy-field";
import { OutLink } from "@/components/out-link";
import { cn } from "@/lib/utils";
import { BACKDROP, POPUP } from "@/workspaces/new-workspace";
import type { Account, AccountId } from "@maxhq/shared/bindings";
import {
	cliLoginBlocked,
	type CliSignInRound,
	loggedOut,
	opensPageItself,
	SEND_AGAIN,
	signedInText,
	signInTitle,
	type SignInPage,
	useCliSignIn,
	wantsDeviceCode,
} from "@maxhq/shared/settings/cli-sign-in";
import { sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { ErrorCopy, FIELD, Label, PRIMARY, Row, SECONDARY, Section, StatusNote } from "./controls";
import { folderAccounts } from "./logins";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

type CliSignInState = {
	/** The account the dialog signs in again, or null when it's closed. */
	readonly account: AccountId | null;
	/** Counts each opening, so every one starts a new round. */
	readonly opened: number;
};

const stores = new WeakMap<Services, StoreApi<CliSignInState>>();

const cliSignInOf = (services: Services): StoreApi<CliSignInState> => {
	let store = stores.get(services);
	if (!store) {
		store = createStore<CliSignInState>(() => ({ account: null, opened: 0 }));
		stores.set(services, store);
	}
	return store;
};

/** Opens the dialog for an account. */
export const openCliSignIn = (services: Services, account: AccountId): void =>
	cliSignInOf(services).setState((s) => ({ account, opened: s.opened + 1 }));

/** Closes it. A round that waits stops. */
export const closeCliSignIn = (services: Services): void =>
	cliSignInOf(services).setState({ account: null });

/** The desktop opens a page in the browser. */
const openPage = (url: string) => {
	window.open(url, "_blank", "noreferrer");
};

// The sign-in page's link, with a copy icon for a browser elsewhere.
const PageLink = ({ url }: { url: string }) => {
	useI18n();
	return (
		<span className="inline-flex items-center gap-0.5">
			<OutLink href={url} className="text-xs">
				{translate("Open the sign-in page")}
			</OutLink>
			<CopyIcon text={url} label={translate("Copy URL")} className="size-[18px]" />
		</span>
	);
};

const Waiting = ({ text }: { text: string }) => {
	useI18n();
	return (
		<span
			role="status"
			className="inline-flex items-center gap-2 text-xs text-muted-foreground"
		>
			<Spinner size={10} />
			{text}
		</span>
	);
};

/** Claude's page shows a code to paste back. */
const PasteCode = ({ page, round }: { page: SignInPage; round: CliSignInRound }) => {
	useI18n();
	const field = useRef<HTMLInputElement>(null);
	const [typed, setTyped] = useState(false);
	const checking = round.state.step === "checking";
	return (
		<form
			className="flex flex-col gap-3"
			onSubmit={(e) => {
				e.preventDefault();
				const input = field.current;
				if (!input || checking) return;
				const code = input.value;
				input.value = "";
				setTyped(false);
				round.finish(code);
			}}
		>
			<p className="text-[12.5px] text-foreground">
				{translate("Your browser opened to sign in. ")}
				<PageLink url={page.url} />
			</p>
			<label className="flex flex-col gap-1.5">
				<Label>{translate("Paste the code from the page")}</Label>
				<input
					ref={field}
					type="password"
					aria-label={translate("Paste the code from the page")}
					autoComplete="off"
					spellCheck={false}
					autoCorrect="off"
					autoCapitalize="off"
					disabled={checking}
					onInput={(e) => setTyped(e.currentTarget.value.trim() !== "")}
					className={cn(FIELD, "font-mono text-xs")}
				/>
			</label>
			<div className="flex items-center justify-end gap-2">
				{checking ? <Waiting text={translate("Checking the code")} /> : null}
				<span className="flex-1" />
				<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
				<button type="submit" className={PRIMARY} disabled={!typed || checking}>
					{translate("Sign in")}
				</button>
			</div>
		</form>
	);
};

/** Codex and Cursor sign in on their page and tell the device themselves. */
const FinishThere = ({ page }: { page: SignInPage }) => {
	useI18n();
	return (
		<div className="flex flex-col gap-3">
			{page.userCode ? (
				<>
					<p className="text-[12.5px] text-foreground">
						{translate("Open the sign-in page and enter this code")}
					</p>
					<div className="flex items-center gap-3">
						<span className="font-mono text-[24px] font-[650] tracking-[0.12em] text-foreground-strong">
							{page.userCode}
						</span>
						<CopyButton text={page.userCode} className={SECONDARY} />
					</div>
					<PageLink url={page.url} />
				</>
			) : (
				<p className="text-[12.5px] text-foreground">
					{translate("Your browser opened to sign in. Finish there. ")}
					<PageLink url={page.url} />
				</p>
			)}
			<div className="flex items-center gap-2">
				<Waiting text={translate("Waiting for you to sign in")} />
				<span className="flex-1" />
				<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
			</div>
		</div>
	);
};

const Round = ({ account }: { account: Account }) => {
	useI18n();
	const { source } = useServices();
	const view = useView((s) => s.view);
	const round = useCliSignIn(source, account, {
		deviceCode: wantsDeviceCode(view, account, "desktop"),
		opens: opensPageItself(view, account, "desktop"),
		open: openPage,
	});
	const { state } = round;
	const device = deviceName(view, account.device);
	return (
		<div className="flex flex-col gap-4">
			<div className="flex flex-col gap-1">
				<Dialog.Title className="text-[15px] font-[650] text-foreground-strong">
					{signInTitle(account.vendor)}
				</Dialog.Title>
				<Dialog.Description className="text-[12.5px] text-muted-foreground">
					{account.label} {translate(" on ")}
					{device}
				</Dialog.Description>
			</div>
			{state.step === "starting" || state.step === "cancelled" ? (
				<div className="flex items-center gap-2">
					<Waiting
						text={translate("Starting the sign-in on {{device}}", { device: device })}
					/>
					<span className="flex-1" />
					<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
				</div>
			) : null}
			{state.step === "waiting" || state.step === "checking" ? (
				state.paste ? (
					<PasteCode page={state} round={round} />
				) : (
					<FinishThere page={state} />
				)
			) : null}
			{state.step === "signed_in" ? (
				<div className="flex flex-col gap-3">
					<p role="status" className="text-[12.5px] text-foreground">
						{signedInText(account)} {SEND_AGAIN}
					</p>
					<div className="flex justify-end">
						<Dialog.Close className={PRIMARY}>{translate("Done")}</Dialog.Close>
					</div>
				</div>
			) : null}
			{state.step === "failed" ? (
				<div className="flex flex-col gap-3">
					<p role="status" className="text-[12.5px] text-status-error-text">
						{state.message}
						<ErrorCopy text={state.message} />
					</p>
					<div className="flex justify-end gap-2">
						<Dialog.Close className={SECONDARY}>{translate("Cancel")}</Dialog.Close>
						<button type="button" className={PRIMARY} onClick={round.begin}>
							{translate("Try again")}
						</button>
					</div>
				</div>
			) : null}
		</div>
	);
};

/** The window's Sign in again dialog, open while it has an account. */
export const CliSignInHost = () => {
	useI18n();
	const services = useServices();
	const store = cliSignInOf(services);
	const id = useStore(store, (s) => s.account);
	const opened = useStore(store, (s) => s.opened);
	const account = useView((s) => (id ? s.view.tables.account.get(id) : undefined));
	return (
		<Dialog.Root
			open={account !== undefined}
			onOpenChange={(open) => {
				if (!open) closeCliSignIn(services);
			}}
		>
			<Dialog.Portal>
				<Dialog.Backdrop className={BACKDROP} />
				<Dialog.Popup className={cn(POPUP, "w-[min(440px,calc(100vw-32px))]")}>
					{account ? <Round key={opened} account={account} /> : null}
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};

/**
 * The Sign in again card on the Claude, Codex and Cursor pages: each login
 * a config folder holds, on any device, with why it can't sign in from
 * here when it can't, and the last usage check's finding when it found it
 * logged out. A logged out login's button says Sign in.
 */
export const SignInAgain = ({ vendor }: { vendor: "claude" | "codex" | "cursor" }) => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const accounts = folderAccounts(view, vendor);
	if (accounts.length === 0) return null;
	return (
		<Section
			title={translate("Sign in again")}
			hint={translate("Runs the login on the device that holds it")}
		>
			{accounts.map((account) => {
				const name = deviceName(view, account.device);
				const blocked = cliLoginBlocked(view, account);
				const failure = loggedOut(account) ? account.usage_failure : null;
				const action = failure ? translate("Sign in") : translate("Sign in again");
				return (
					<Row
						key={account.id}
						title={translate("{{label}} on {{name}}", {
							label: account.label,
							name: name,
						})}
						sub={blocked ?? account.email ?? undefined}
					>
						<StatusNote
							note={failure ? { text: sentence(failure.reason), error: true } : null}
						/>
						<button
							type="button"
							className={SECONDARY}
							disabled={blocked !== null}
							aria-label={translate("{{action}} to {{label}} on {{name}}", {
								action: action,
								label: account.label,
								name: name,
							})}
							onClick={() => openCliSignIn(services, account.id)}
						>
							{action}
						</button>
					</Row>
				);
			})}
		</Section>
	);
};
