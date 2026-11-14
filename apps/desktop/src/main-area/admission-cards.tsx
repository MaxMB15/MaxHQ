// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The cards a device raised about programs that connected without a key it
// trusts and asked for more, on the device's page. Each lays out what the
// device knows about the program, with Don't allow first. This app proved
// its key, so its answer counts.

import { useI18n } from "@maxhq/shared/i18n";

import { useServices } from "@/app/services";
import { ConsentCard } from "@/components/consent-card";
import type { Admission, DeviceId } from "@maxhq/shared/bindings";
import { admissionCard, openAdmissions } from "@maxhq/shared/ui/admission";
import type { ViewState } from "@maxhq/shared/view/replica";

import { useAnswer } from "./device-identity";

const AdmissionCard = ({ admission, view }: { admission: Admission; view: ViewState }) => {
	const { t } = useI18n();

	const { source } = useServices();
	const { busy, note, run } = useAnswer();
	const card = admissionCard(view, admission);
	const answer = (allow: boolean) => void run(() => source.answerAdmission(admission.id, allow));
	return (
		<ConsentCard
			title={card.title}
			risk={card.risk}
			details={card.facts.length > 0 ? [{ title: t("The program"), facts: card.facts }] : []}
			remember={card.remember}
			note={note}
			busy={busy}
			answers={[
				{ label: card.deny, primary: true, onAnswer: () => answer(false) },
				{ label: card.allow, onAnswer: () => answer(true) },
			]}
		/>
	);
};

/** The open cards `device` raised, oldest first. */
export const AdmissionCards = ({ device, view }: { device: DeviceId; view: ViewState }) => {
	useI18n();
	return (
		<>
			{openAdmissions(view, device).map((admission) => (
				<AdmissionCard key={admission.id} admission={admission} view={view} />
			))}
		</>
	);
};
