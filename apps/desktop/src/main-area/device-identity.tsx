// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which device a link reaches. A link whose hello names a device another
// link already knows waits here for your answer, with both links' details;
// one you said is a different device keeps a note and a way to ask again;
// and a link that reaches its device can forget it, so MaxHQ asks again.

import { t, useI18n } from "@maxhq/shared/i18n";

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { useServices } from "@/app/services";
import { ConsentCard } from "@/components/consent-card";
import { ConfirmButton, Row, SECONDARY, Section, StatusNote } from "@/settings/controls";
import { failureOf, type Note } from "@/settings/result";
import type { DeviceLink, Timestamp } from "@maxhq/shared/bindings";
import type { ActionResult, CommandResult } from "@maxhq/shared/data/source";
import {
	claimCard,
	claimNames,
	differentNote,
	FORGET_IDENTITY,
	forgetNote,
} from "@maxhq/shared/ui/link-claim";
import type { ViewState } from "@maxhq/shared/view/replica";

/** Runs one answer on a card, with a note while it goes and when it fails. */
export const useAnswer = () => {
	const [busy, setBusy] = useState(false);
	const [note, setNote] = useState<Note | null>(null);
	const run = async (call: () => Promise<ActionResult | CommandResult>) => {
		setBusy(true);
		setNote(null);
		try {
			const failure = failureOf(await call());
			if (failure) setNote({ text: failure, error: true });
		} catch (error) {
			console.warn("Couldn't answer a card", error);
			setNote({ text: t("Couldn't reach the app. Try again."), error: true });
		} finally {
			setBusy(false);
		}
	};
	return { busy, note, run };
};

/** The question a held link waits on, or nothing when it isn't waiting. */
export const LinkClaimCard = ({
	link,
	view,
	now,
}: {
	link: DeviceLink;
	view: ViewState;
	now: Timestamp;
}) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAnswer();
	const asking = link.claim?.state === "asking";
	const details = useQuery({
		queryKey: ["link-claim", link.id, link.claim?.since ?? null],
		queryFn: () => source.linkClaimDetails(link.id),
		enabled: asking,
	});
	const names = claimNames(view, link);
	if (!names || !asking) return null;
	const result = details.data;
	const card = claimCard(names, result?.status === "ok" ? result.data : null, now);
	const failed = result?.status === "error" ? failureOf(result) : null;
	return (
		<ConsentCard
			title={card.title}
			risk={card.risk}
			details={card.sides}
			remember={card.remember}
			note={note}
			busy={busy}
			answers={[
				{
					label: card.different,
					onAnswer: () => void run(() => source.answerLinkClaim(link.id, false)),
				},
				{
					label: card.same,
					onAnswer: () => void run(() => source.answerLinkClaim(link.id, true)),
				},
			]}
		>
			{details.isPending ? (
				<p className="text-[11.5px] text-muted-foreground">
					{t("Reading both links' details")}
				</p>
			) : failed ? (
				<p className="selectable text-[11.5px] text-status-error-text">{failed}</p>
			) : null}
		</ConsentCard>
	);
};

/** Which device an SSH link reaches, with a way to forget it or ask again. */
export const DeviceIdentity = ({ link, view }: { link: DeviceLink; view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAnswer();
	if (link.route.type !== "ssh") return null;
	const names = claimNames(view, link);
	if (names && !names.different) return null;
	const forget = (
		<ConfirmButton
			label={t(FORGET_IDENTITY)}
			question={t("MaxHQ asks again at its next hello.")}
			confirm={t("Forget")}
			disabled={busy}
			onConfirm={() => void run(() => source.forgetLinkIdentity(link.id))}
		/>
	);
	return (
		<Section title={t("Identity")} label={t("Identity of {{name}}", { name: link.name })}>
			{names ? (
				<Row title={differentNote(names)}>
					<StatusNote note={note} />
					<button
						type="button"
						className={SECONDARY}
						disabled={busy}
						onClick={() => void run(() => source.askLinkClaimAgain(link.id))}
					>
						{t("Ask again")}
					</button>
					{forget}
				</Row>
			) : link.device ? (
				<Row
					title={forgetNote(
						link.name,
						view.tables.device.get(link.device)?.name ?? link.name
					)}
				>
					<StatusNote note={note} />
					{forget}
				</Row>
			) : (
				<Row
					title={t("{{name}} hasn't named its device yet", { name: link.name })}
					sub={t("MaxHQ learns it at the link's first hello.")}
				/>
			)}
		</Section>
	);
};
