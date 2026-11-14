// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Which codes the scan screen pairs with. The Mac's Add phone shows one that
// starts "maxhq:". Anything else the camera sees, such as a menu's link, is
// passed over with a hint. The same code opened as a link, as the Camera
// app opens it, goes to the scan screen.

import { t } from "@maxhq/shared/i18n";

const PREFIX = "maxhq:";

/** The pairing code in what was scanned or pasted, or null when it isn't one. */
export const pairCodeOf = (text: string): string | null => {
	const code = text.trim();
	return code.startsWith(PREFIX) && code.length > PREFIX.length ? code : null;
};

/** What the scan screen says about something that isn't a pairing code. */
export const NOT_A_CODE = "That isn't a MaxHQ code. Scan the one on the Mac's Add phone.";

/**
 * What the camera reading `text` leads to: a code to confirm, or a hint and
 * more scanning. It never pairs by itself, since any code in front of the
 * camera could be a stranger's.
 */
export const afterScan = (
	text: string
): { readonly confirm: string } | { readonly hint: string } => {
	const code = pairCodeOf(text);
	return code ? { confirm: code } : { hint: t(NOT_A_CODE) };
};

/**
 * The app link for a URL iOS opened MaxHQ with. A pairing code becomes a
 * link to the scan screen with the code in its paste box; any other URL,
 * such as maxhq://agent/<id>, stays as it is.
 */
export const scanLinkOf = (url: string): string => {
	const code = url.startsWith(`${PREFIX}//`) ? null : pairCodeOf(url);
	return code ? `${PREFIX}//scan?code=${encodeURIComponent(code)}` : url;
};
