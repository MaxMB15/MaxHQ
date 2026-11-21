// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The Above the conversation card: whether an agent's view offers
// to forward or open the ports its processes start listening on. With it
// off, the header's ports popover still lists them with Forward and Open.

import { useServices } from "@/app/services";
import { Switch } from "@/components/ui/switch";
import { setPortPrompts, usePortPrompts } from "@/ports/prompts-prefs";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { Row, Section } from "./controls";

/** The card's headings and labels, for the settings search. */
export const PORT_PROMPTS_TERMS: readonly string[] = ["Conversation", "Port prompts"];

/** The Above the conversation card. */
export const PortPromptsSection = () => {
	useI18n();
	const services = useServices();
	const shown = usePortPrompts();
	const about = translate("Offer to forward or open a port an agent starts listening on.");
	return (
		<Section title={translate("Above the conversation")}>
			<Row
				title={translate("Port prompts")}
				sub={
					shown
						? about
						: `${about} ${translate("Off. Ports still show under the device chip in each agent's header, with Forward and Open there.")}`
				}
			>
				<Switch
					aria-label={translate("Port prompts")}
					checked={shown}
					onCheckedChange={(on) => setPortPrompts(services, on)}
				/>
			</Row>
		</Section>
	);
};
