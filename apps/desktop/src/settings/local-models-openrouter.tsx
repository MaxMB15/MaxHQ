// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// OpenRouter in the Servers card: the choice at the top of the form that
// adds a server, OpenRouter's part of that form, and Sign in again on an
// OpenRouter server's row. Sign in with OpenRouter runs a whole browser
// round through the source, so these only start it, say it waits, and say
// how it ended. A round that's still waiting when its form or row goes
// away is cancelled. Pasting a key instead is the shared form's job.

import { useEffect, useRef, useState } from "react";

import { useServices, useView } from "@/app/services";
import { OutLink } from "@/components/out-link";
import { cn } from "@/lib/utils";
import type { DeviceId, ModelServer } from "@maxhq/shared/bindings";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import {
	canSignInWithOpenRouter,
	isOpenRouter,
	OPENROUTER_KEYS_URL,
	OPENROUTER_LABEL,
	OPENROUTER_URL,
	type OpenRouterRound,
	openRouterServerOf,
	replacesKeyText,
	roundBroke,
	roundEnded,
	signedInAgainText,
	spendingLimit,
	updateToSignInText,
	waitingText,
} from "@maxhq/shared/local-models/openrouter";
import type { ViewState } from "@maxhq/shared/view/replica";

import { FIELD, Label, PRIMARY, SECONDARY, SEG, SEG_ITEM } from "./controls";

/** The kinds of server the form adds. */
export type ServerKind = "own" | "openrouter";

/** The choice at the top of the form: your own server at a URL, or OpenRouter. */
export const ServerChoice = ({
	value,
	onChange,
}: {
	value: ServerKind;
	onChange: (kind: ServerKind) => void;
}) => {
	useI18n();
	const item = (kind: ServerKind, label: string) => (
		<button
			type="button"
			aria-pressed={value === kind}
			className={SEG_ITEM}
			onClick={() => onChange(kind)}
		>
			{label}
		</button>
	);
	return (
		<div role="group" aria-label={translate("Kind of server")} className={SEG}>
			{item("own", translate("Your own server"))}
			{item("openrouter", OPENROUTER_LABEL)}
		</div>
	);
};

/**
 * A Sign in with OpenRouter round on a device: where it is, Start and
 * Cancel. One cancelled comes back to the start with nothing to say, and
 * one still waiting when the form or row goes away is cancelled.
 */
const useRound = (device: DeviceId) => {
	const { source } = useServices();
	const [round, setRound] = useState<OpenRouterRound>({ state: "idle" });
	const waiting = useRef(false);
	const cancelled = useRef(false);
	useEffect(
		() => () => {
			if (waiting.current) void source.cancelSignIn();
		},
		[source]
	);

	const start = async (limitCents: number | null): Promise<OpenRouterRound> => {
		cancelled.current = false;
		waiting.current = true;
		setRound({ state: "waiting" });
		let ended: OpenRouterRound;
		try {
			ended = roundEnded(
				await source.signInOpenRouter(device, limitCents),
				cancelled.current
			);
		} catch (error) {
			console.warn(translate("Couldn't sign in with OpenRouter"), error);
			ended = roundBroke();
		} finally {
			waiting.current = false;
		}
		setRound(ended);
		return ended;
	};

	const cancel = () => {
		cancelled.current = true;
		void source.cancelSignIn();
	};

	return { round, start, cancel };
};

/** A round's line while it waits, or why it failed. */
const RoundLine = ({ round }: { round: OpenRouterRound }) => {
	useI18n();
	if (round.state === "waiting") {
		return (
			<span role="status" className="text-muted-foreground">
				{waitingText()}
			</span>
		);
	}
	if (round.state === "failed") {
		return (
			<span role="status" className="text-status-error-text">
				{round.reason}
			</span>
		);
	}
	return null;
};

/**
 * The button that stops the round that waits. It names the sign-in, since
 * the form's own Cancel sits below it.
 */
const CancelRound = ({ onCancel }: { onCancel: () => void }) => {
	useI18n();
	return (
		<button type="button" className={SECONDARY} onClick={onCancel}>
			{translate("Cancel the sign-in")}
		</button>
	);
};

/**
 * OpenRouter's part of the form that adds a server, above its key field:
 * the URL, which is set, and Sign in with OpenRouter with an optional
 * spending limit. A device whose maxd can't sign in says to update it.
 * Either way it ends with the heading of the key field below, and where to
 * get a key.
 */
export const OpenRouterSignIn = ({
	device,
	view,
	onSignedIn,
}: {
	device: DeviceId;
	view: ViewState;
	/** Called once a round gave the device a key. */
	onSignedIn: () => void;
}) => {
	useI18n();
	const can = canSignInWithOpenRouter(view, device);
	const existing = openRouterServerOf(view, device);
	const [typed, setTyped] = useState("");
	const limit = spendingLimit(typed);
	const { round, start, cancel } = useRound(device);
	const waiting = round.state === "waiting";

	const signIn = async () => {
		if (!limit.ok) return;
		const ended = await start(limit.cents);
		if (ended.state === "signed_in") onSignedIn();
	};

	return (
		<>
			<div className="flex flex-col gap-1.5 text-xs">
				<Label>{translate("URL")}</Label>
				<span className="selectable font-mono text-muted-foreground">{OPENROUTER_URL}</span>
				{existing && can ? (
					<span className="text-muted-foreground">{replacesKeyText(existing)}</span>
				) : null}
			</div>
			{can ? (
				<div className="flex flex-col gap-1.5 text-xs">
					<div className="flex items-end gap-3">
						<label className="flex flex-col gap-1.5">
							<Label>{translate("Spending limit")}</Label>
							<span className="relative w-40">
								<span
									aria-hidden
									className="pointer-events-none absolute inset-y-0 left-2.5 flex items-center font-mono text-muted-foreground"
								>
									$
								</span>
								<input
									aria-label={translate("Spending limit")}
									aria-invalid={!limit.ok}
									value={typed}
									placeholder={translate("No limit")}
									onChange={(e) => setTyped(e.target.value)}
									disabled={waiting}
									inputMode="decimal"
									spellCheck={false}
									autoComplete="off"
									className={cn(FIELD, "pl-6 font-mono text-xs")}
								/>
							</span>
						</label>
						<span className="ml-auto flex items-center gap-2.5">
							{waiting ? (
								<CancelRound onCancel={cancel} />
							) : (
								<button
									type="button"
									className={PRIMARY}
									disabled={!limit.ok}
									onClick={() => void signIn()}
								>
									{round.state === "failed"
										? translate("Try again")
										: translate("Sign in with OpenRouter")}
								</button>
							)}
						</span>
					</div>
					<span className={limit.ok ? "text-muted-foreground" : "text-status-error-text"}>
						{limit.ok
							? translate("OpenRouter stops this key at the limit")
							: limit.problem}
					</span>
					<RoundLine round={round} />
				</div>
			) : (
				<p className="text-xs text-muted-foreground">{updateToSignInText()}</p>
			)}
			<div className="flex items-center gap-3 border-t border-sidebar-border pt-3">
				<span className="text-[12.5px] text-foreground">
					{can ? translate("Or paste an API key") : translate("Paste an API key")}
				</span>
				<OutLink href={OPENROUTER_KEYS_URL} className="ml-auto text-xs">
					{translate("Get a key")}
				</OutLink>
			</div>
		</>
	);
};

/**
 * Sign in again on a server's row, for OpenRouter's on a device whose maxd
 * can: a round that gives the server a new key and keeps its ID, so the
 * agents on it go on. Its line goes under the row's words and its button
 * beside Test; both are null on any other server.
 */
export const useSignInAgain = (
	device: DeviceId,
	server: Pick<ModelServer, "kind" | "base_url">,
	disabled: boolean
) => {
	const view = useView((s) => s.view);
	const { round, start, cancel } = useRound(device);
	if (!isOpenRouter(server) || !canSignInWithOpenRouter(view, device)) {
		return { line: null, button: null };
	}
	const line =
		round.state === "signed_in" ? (
			<span role="status" className="text-muted-foreground">
				{signedInAgainText()}
			</span>
		) : (
			<RoundLine round={round} />
		);
	const button =
		round.state === "waiting" ? (
			<CancelRound onCancel={cancel} />
		) : (
			<button
				type="button"
				className={SECONDARY}
				disabled={disabled}
				onClick={() => void start(null)}
			>
				{translate("Sign in again")}
			</button>
		);
	return { line, button };
};
