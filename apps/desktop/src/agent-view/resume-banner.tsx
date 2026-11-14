// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The banner over the composer once an agent MaxHQ holds has stopped: you
// stopped it, it kept making no progress, or its CLI exited. Resume starts
// it again on the same conversation, and so does sending it a message.
//
// A local ACP agent that needs a sign-in gets the sign-in card here
// instead: the ways it says it signs in, its command to copy so you can
// sign in from a terminal on its device, and Try again, which starts it
// again. A local agent that can't load a session warns that its history
// doesn't come back.

import { Pause } from "lucide-react";
import { useState } from "react";

import { useServices, useView } from "@/app/services";
import { CopyButton } from "@/components/copy-button";
import type { Account, Agent } from "@maxhq/shared/bindings";
import { cn } from "@/lib/utils";
import {
	agentOfAccount,
	historyWarning,
	keepsHistory,
	needsSignIn,
	signInCommand,
} from "@maxhq/shared/settings/acp-agents";
import { sentence } from "@maxhq/shared/status/format";
import { deviceName } from "@maxhq/shared/view/select";

import { failureOf, type SendState } from "@maxhq/shared/agent-view/answer";
import { Spinner } from "./icons";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

/** Whether an agent MaxHQ holds has stopped, so Resume can start it again. */
export const canResume = (agent: Agent): boolean => {
	if (agent.control === "observed" || agent.parent !== null) return false;
	if (agent.activity.state === "paused") return true;
	// On Resume, maxd starts a Claude CLI that exited. It doesn't do that
	// for a Codex agent that failed, so that one gets no banner.
	return (
		agent.activity.state === "error" && agent.vendor === "claude" && agent.link.state === "down"
	);
};

const wording = (agent: Agent, account: Account | undefined): { title: string; body: string } => {
	const { title, body } = stopWording(agent);
	return keepsHistory(account) || !account
		? { title, body }
		: { title, body: `${body} ${historyWarning(account.label)}` };
};

const stopWording = (agent: Agent): { title: string; body: string } => {
	const { activity } = agent;
	if (activity.state === "error") {
		return {
			title: translate("Stopped with an error"),
			body: `${sentence(activity.reason)}. The conversation stays, and Resume starts it again.`,
		};
	}
	if (activity.state === "paused" && activity.reason === "resumes_made_no_progress") {
		return {
			title: translate("Paused after 3 resumes"),
			body: translate(
				"It was resumed 3 times in a row without making progress. Look over where it got to, then resume it."
			),
		};
	}
	return {
		title: translate("Stopped"),
		body: translate(
			"The conversation stays. Resume starts it again, and so does sending it a message."
		),
	};
};

/** Over the composer of a stopped agent that MaxHQ holds, or one that needs a sign-in. */
export const ResumeBanner = ({ agent }: { agent: Agent }) => {
	useI18n();
	const { source } = useServices();
	const [state, setState] = useState<SendState>({ state: "idle" });
	const account = useView((s) =>
		agent.account ? s.view.tables.account.get(agent.account) : undefined
	);
	const definition = useView((s) => (account ? agentOfAccount(s.view, account) : undefined));
	const where = useView((s) => (account ? deviceName(s.view, account.device) : ""));
	const signIn = account !== undefined && needsSignIn(agent, account);
	const { title, body } = signIn
		? {
				title: translate("{{label}} needs a sign-in", { label: account.label }),
				body: translate(
					"Sign in from a terminal on {{where}} the way {{label}} asks, then try again. MaxHQ keeps no login for it.",
					{ where: where, label: account.label }
				),
			}
		: wording(agent, account);
	const methods = signIn ? (account.acp?.auth ?? []) : [];

	const resume = () => {
		setState({ state: "sending" });
		source.continueHere(agent.id).then(
			(result) => {
				const failure = failureOf(result);
				setState(failure ? { state: "failed", message: failure } : { state: "sent" });
			},
			(error: unknown) =>
				setState({
					state: "failed",
					message:
						error instanceof Error
							? error.message
							: translate("Couldn't resume the agent."),
				})
		);
	};

	return (
		<div className="shrink-0 px-7 pb-2">
			<section
				aria-label={
					signIn ? translate("Agent needs a sign-in") : translate("Agent stopped")
				}
				className="mx-auto flex max-w-[744px] items-center gap-[11px] rounded-[10px] border border-input bg-card py-2.5 pr-3 pl-3.5"
			>
				<span
					aria-hidden
					className="grid size-[26px] shrink-0 place-items-center rounded-md border border-input bg-hover-card text-secondary-foreground"
				>
					<Pause size={13} strokeWidth={2.2} />
				</span>
				<div className="flex min-w-0 flex-1 flex-col gap-[3px]">
					<h3 className="text-[13px] font-semibold text-foreground-strong">{title}</h3>
					<p className="text-[12px] leading-[1.45] text-muted-foreground">{body}</p>
					{methods.length > 0 ? (
						<ul className="flex flex-col gap-0.5 text-[12px] leading-[1.45] text-muted-foreground">
							{methods.map((m) => (
								<li key={m.id}>
									<span className="font-medium text-secondary-foreground">
										{m.name}
									</span>
									{m.description ? `: ${m.description}` : null}
								</li>
							))}
						</ul>
					) : null}
					{state.state === "failed" ? (
						<p role="alert" className="text-[11.5px] text-status-error-text">
							{state.message}
						</p>
					) : null}
				</div>
				{signIn && definition ? (
					<CopyButton
						text={signInCommand(definition)}
						label={translate("Copy command")}
						title={signInCommand(definition)}
						className="inline-flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md border border-input px-2.5 text-[12px] font-medium whitespace-nowrap text-secondary-foreground hover:bg-accent"
					/>
				) : null}
				<button
					type="button"
					disabled={state.state === "sending" || state.state === "sent"}
					onClick={resume}
					className={cn(
						"inline-flex h-7 shrink-0 cursor-pointer items-center gap-1.5 rounded-md bg-primary px-2.5 text-[12px] font-semibold whitespace-nowrap text-primary-foreground shadow-lit",
						"hover:bg-primary/90 disabled:cursor-default disabled:opacity-60"
					)}
				>
					{state.state === "sending" ? (
						<Spinner
							size={11}
							className="border-primary-foreground/30 border-t-primary-foreground"
						/>
					) : null}
					{signIn ? translate("Try again") : translate("Resume")}
				</button>
			</section>
		</div>
	);
};
