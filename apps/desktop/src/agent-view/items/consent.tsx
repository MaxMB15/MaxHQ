// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A card that asks for your consent: what's asked for, the risk in plain
// words, what changed since you last allowed it, the details to judge it
// by, and each thing it covers under its heading, with a warning where it
// can do more than it says. A value that looks secret stays hidden until
// you show it. Where the answer can be kept, a box keeps it and says where
// to change it later. maxd asks it itself, so it takes an answer even in a
// session MaxHQ only watches. A card about a kind of file, such as git
// hooks, shows in the reader's language and still sends maxd's labels.

import { ShieldAlert, TriangleAlert } from "lucide-react";
import { useCallback, useState } from "react";

import type { AttentionAnswer, Consent, ConsentRow, Question } from "@maxhq/shared/bindings";
import { useSend } from "@maxhq/shared/agent-view/answer";
import { consentAnswer, consentGroups, shownValue } from "@maxhq/shared/agent-view/consent";
import { shownConsent, shownOption } from "@maxhq/shared/agent-view/edit-kinds";
import { rowKind, useRow } from "@maxhq/shared/agent-view/context";
import { useKept } from "@maxhq/shared/agent-view/kept";
import { sentence } from "@maxhq/shared/status/format";
import { ConsentFacts } from "@/components/consent-facts";
import { RememberCheck } from "@/components/remember-check";
import { cn } from "@/lib/utils";

import { PrimaryButton, SecondaryButton, SendNote, WaitingCard } from "./card";
import type { ItemOf } from "./types";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const ICON = <ShieldAlert size={15} strokeWidth={1.9} aria-hidden />;

const Row = ({ row }: { row: ConsentRow }) => {
	useI18n();
	const [shown, setShown] = useState(false);
	return (
		<li className="flex min-w-0 flex-col gap-0.5">
			<div className="flex min-w-0 items-start gap-2">
				<code className="min-w-0 flex-1 font-mono text-[12px] break-all text-foreground select-text">
					{row.name}
					<span className="text-muted-foreground">=</span>
					<span className="text-secondary-foreground">{shownValue(row, shown)}</span>
				</code>
				{row.secret ? (
					<button
						type="button"
						aria-label={translate("{{value1}} the value of {{name}}", {
							value1: shown ? translate("Hide") : translate("Show"),
							name: row.name,
						})}
						aria-pressed={shown}
						onClick={() => setShown(!shown)}
						className="shrink-0 cursor-pointer text-[11.5px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
					>
						{shown ? translate("Hide") : translate("Show")}
					</button>
				) : null}
			</div>
			{row.warning ? (
				<p
					className={cn(
						"flex items-start gap-1 text-[11.5px]",
						row.strong ? "font-medium text-status-error-text" : "text-status-busy-text"
					)}
				>
					<TriangleAlert
						size={12}
						strokeWidth={2}
						aria-hidden
						className="mt-0.5 shrink-0"
					/>
					<span>{sentence(row.warning)}.</span>
				</p>
			) : null}
		</li>
	);
};

const Body = ({ consent }: { consent: Consent }) => {
	useI18n();
	return (
		<div className="flex flex-col gap-2.5">
			<p className="text-[12.5px] leading-normal text-foreground select-text">
				{consent.risk}
			</p>
			{consent.changes.length > 0 ? (
				<div className="flex flex-col gap-0.5">
					<span className="text-[11.5px] font-medium text-status-ready-text">
						{translate("What changed since you last allowed them")}
					</span>
					<ul className="flex flex-col text-[12px] text-secondary-foreground">
						{consent.changes.map((line) => (
							<li key={line} className="font-mono break-all select-text">
								{line}
							</li>
						))}
					</ul>
				</div>
			) : null}
			{consent.facts?.length ? <ConsentFacts facts={consent.facts} /> : null}
			{consent.rows.length > 0 ? (
				<div className="flex max-h-[50vh] flex-col gap-2 overflow-y-auto rounded-md border border-border bg-background px-2.5 py-2">
					{consentGroups(consent.rows).map((group) => (
						<section
							key={group.name}
							aria-label={group.name}
							className="flex flex-col gap-1"
						>
							<h4 className="text-[11.5px] font-medium text-muted-foreground">
								{group.name}
							</h4>
							<ul className="flex flex-col gap-1.5">
								{group.rows.map((row) => (
									<Row key={row.name} row={row} />
								))}
							</ul>
						</section>
					))}
				</div>
			) : null}
		</div>
	);
};

/** An open question that carries a consent card, laid out as one. */
export const ConsentCard = ({
	item,
	question,
	consent: sent,
}: {
	item: ItemOf<"question">;
	question: Question;
	consent: Consent;
}) => {
	useI18n();
	const consent = shownConsent(sent);
	const { answer } = useRow();
	const kind = rowKind(useRow());
	const [remember, setRemember] = useKept(`remember:${item.attention}`, false);
	const send = useCallback(
		(a: AttentionAnswer) => answer(item.attention, a),
		[answer, item.attention]
	);
	const [state, submit] = useSend(send);
	const busy = state.state === "sending";
	const [primary, ...others] = question.options;
	const pick = (label: string) => submit(consentAnswer(question, label, remember));

	return (
		<WaitingCard
			label={translate("Consent request")}
			icon={ICON}
			title={consent.title}
			kind={kind}
			since={item.started_at}
		>
			<Body consent={consent} />
			{consent.remember ? (
				<RememberCheck
					label={consent.remember.label}
					where={consent.remember.settings}
					checked={remember}
					onChange={setRemember}
					disabled={busy}
				/>
			) : null}
			<div className="flex flex-wrap items-center gap-1.5">
				{primary ? (
					<PrimaryButton
						hint={false}
						busy={busy}
						disabled={busy}
						onClick={() => pick(primary.label)}
					>
						{shownOption(sent, primary.label)}
					</PrimaryButton>
				) : null}
				{others.map((option) => (
					<SecondaryButton
						key={option.label}
						disabled={busy}
						onClick={() => pick(option.label)}
					>
						{shownOption(sent, option.label)}
					</SecondaryButton>
				))}
			</div>
			<SendNote state={state} kind={kind} />
		</WaitingCard>
	);
};
