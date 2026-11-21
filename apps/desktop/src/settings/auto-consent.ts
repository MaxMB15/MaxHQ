// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The question every mode picker asks the first time you pick Auto, and
// your answer, kept in localStorage so the app asks once per install. A
// start or a message in an Auto you didn't pick asks it too, with Ask
// first instead as the other answer.

import type { Services } from "@/app/services";
import { requestAnswer, requestConfirm } from "@/components/confirm";
import type { Agent, DeviceId, PermissionMode } from "@maxhq/shared/bindings";
import {
	type AgreedMode,
	agreeOnMode,
	agreeToSend,
	AUTO_DEFAULT_QUESTION,
	AUTO_QUESTION,
	type AutoChoice,
	keptConsent,
	type SendGate,
	whenModeAllowed,
} from "@maxhq/shared/settings/auto-mode";
import { t } from "@maxhq/shared/i18n";

/** The storage key for whether you confirmed Auto. */
export const AUTO_CONSENT_KEY = "maxhq.auto-mode.confirmed";

const consent = keptConsent({
	read: () => localStorage.getItem(AUTO_CONSENT_KEY),
	write: (value) => localStorage.setItem(AUTO_CONSENT_KEY, value),
});

/** Whether you confirmed Auto on this Mac, so a pick of it no longer asks. */
export const autoConfirmed = (): boolean => consent.confirmed();

/** Forgets that you confirmed Auto, so the next pick of it asks again. */
export const forgetAutoConsent = (): void => {
	try {
		localStorage.removeItem(AUTO_CONSENT_KEY);
	} catch (error) {
		console.warn(t("Couldn't forget the Auto mode answer"), error);
	}
};

/** Runs `go` to switch to `mode`. Auto asks first, until you confirm it once. */
export const pickMode = (services: Services, mode: PermissionMode, go: () => void): void =>
	whenModeAllowed(
		mode,
		consent,
		(yes) =>
			void requestConfirm(services, AUTO_QUESTION).then((ok) => {
				if (ok) yes();
			}),
		go
	);

/** Asks `AUTO_DEFAULT_QUESTION` in the app's dialog. */
const askAutoDefault = async (services: Services): Promise<AutoChoice | null> => {
	const pressed = await requestAnswer(services, AUTO_DEFAULT_QUESTION);
	if (pressed === null) return null;
	return pressed === "action" ? "auto" : "ask";
};

/**
 * The mode to start an agent on `device` in, set to `mode` without your
 * pick, as `agreeOnMode` settles it.
 */
export const agreedMode = (
	services: Services,
	mode: PermissionMode,
	device: DeviceId
): Promise<AgreedMode> =>
	agreeOnMode(mode, consent, () => askAutoDefault(services), {
		source: services.source,
		view: services.view.getState().view,
		device,
		agent: null,
	});

/** Whether a message to `agent` goes, as `agreeToSend` settles it. */
export const agreedSend = (services: Services, agent: Agent): Promise<SendGate> =>
	agreeToSend(
		agent,
		consent,
		() => askAutoDefault(services),
		services.source,
		services.view.getState().view
	);
