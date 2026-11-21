// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The answers this Mac keeps to the questions that ask for your consent,
// with Forget, which makes the next one ask again. Today that's whether
// you confirmed Auto mode.

import { useState } from "react";

import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { autoConfirmed, forgetAutoConsent } from "../auto-consent";
import { Row, SECONDARY, Section } from "../controls";

/** The card's labels, for the settings search. */
export const REMEMBERED_TERMS: readonly string[] = ["Auto mode", "Forget"];

/** The Remembered answers card. */
export const RememberedAnswersCard = () => {
	useI18n();
	const [confirmed, setConfirmed] = useState(autoConfirmed);
	return (
		<Section title={translate("Remembered answers")}>
			<Row
				title={translate("Auto mode")}
				sub={
					confirmed
						? translate("You confirmed it, so picking it no longer asks.")
						: translate("Picking it asks first.")
				}
			>
				<button
					type="button"
					className={SECONDARY}
					disabled={!confirmed}
					aria-label={translate("Forget the Auto mode answer")}
					onClick={() => {
						forgetAutoConsent();
						setConfirmed(autoConfirmed());
					}}
				>
					{translate("Forget")}
				</button>
			</Row>
		</Section>
	);
};
