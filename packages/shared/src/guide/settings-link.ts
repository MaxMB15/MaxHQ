// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Where in Settings a link goes. Settings is one page of groups and cards,
// with a tab for each device and one for phones, and a few views of their
// own: the logins, a project's settings, the skills library, scheduled
// tasks and local models. Links from before, such as
// maxhq://settings/editor or maxhq://device/{device}/idle, name the pages
// and device cards there were then. The tables here send each of them to
// where it lives now, so an old link in a note or an answer still works.

import type { LoginPage } from "../settings/login-steps";

/** The groups of the Settings page, in order. */
export const SETTINGS_GROUPS = [
	"general",
	"chat",
	"notifications",
	"accounts",
	"devices",
	"permissions",
	"projects",
] as const;

export type SettingsGroup = (typeof SETTINGS_GROUPS)[number];

/** The cards on the Settings page, outside the device tabs, in order. */
export const PAGE_CARDS = [
	"appearance",
	"updates",
	"composer",
	"open-in",
	"above-conversation",
	"sidebar",
	"dispatcher",
	"notifications",
	"badge",
	"accounts",
	"cursor-usage",
	"other-agents",
	"grants",
	"new-permissions",
	"claude-artifacts",
	"access-keys",
	"app-keys",
	"remembered-answers",
	"projects",
] as const;

export type PageCard = (typeof PAGE_CARDS)[number];

/** The cards of a device's tab, in order. */
export const DEVICE_CARDS = [
	"connection",
	"new-agents",
	"inbox",
	"resources",
	"terminals",
	"setup-tokens",
	"workspaces",
	"library",
	"network",
	"test-machines",
] as const;

export type DeviceCardId = (typeof DEVICE_CARDS)[number];

/** The cards of the Phones tab. */
export const PHONE_CARDS = ["phones", "apns"] as const;

export type PhoneCard = (typeof PHONE_CARDS)[number];

/** The logins that open as views of their own. The Apple push key is on the Phones tab. */
export type LoginView = Exclude<LoginPage, "apns">;

export const LOGIN_VIEWS: readonly LoginView[] = [
	"claude",
	"codex",
	"cursor",
	"openrouter",
	"github",
];

/** A project's tabs in Settings. */
export type SettingsProjectTab = "general" | "skills";

/** A view of its own inside Settings, in place of the page, with a Back button. */
export type SettingsDetail =
	| { readonly kind: "login"; readonly login: LoginView; readonly device?: string }
	| { readonly kind: "project"; readonly project: string; readonly tab?: SettingsProjectTab }
	| { readonly kind: "skills" }
	| { readonly kind: "scheduled-tasks" }
	| { readonly kind: "local-models"; readonly device?: string };

/** A place in Settings. */
export type SettingsPlace =
	| SettingsDetail
	/** The page, at a group or a card, or at the top. */
	| { readonly kind: "page"; readonly at: SettingsGroup | PageCard | null }
	/** A device's tab, at a card or the tab's top. No device means this Mac. */
	| {
			readonly kind: "device";
			readonly device: string | null;
			readonly card: DeviceCardId | null;
	  }
	/** The Phones tab. */
	| { readonly kind: "phones"; readonly card: PhoneCard | null }
	/** The keyboard shortcuts, which show over any view. */
	| { readonly kind: "shortcuts" };

const page = (at: SettingsGroup | PageCard | null): SettingsPlace => ({ kind: "page", at });
const thisMac = (card: DeviceCardId | null): SettingsPlace => ({
	kind: "device",
	device: null,
	card,
});

/** Where each page of the Settings from before went. */
export const OLD_SETTINGS_PAGES: Readonly<Record<string, SettingsPlace>> = {
	appearance: page("appearance"),
	sidebar: page("sidebar"),
	editor: page("open-in"),
	dispatcher: page("dispatcher"),
	"agent-tools": page("permissions"),
	"local-models": { kind: "local-models" },
	"scheduled-tasks": { kind: "scheduled-tasks" },
	"test-hosts": thisMac("test-machines"),
	skills: { kind: "skills" },
	notifications: page("notifications"),
	updates: page("updates"),
	phones: { kind: "phones", card: "phones" },
	shortcuts: { kind: "shortcuts" },
	claude: { kind: "login", login: "claude" },
	codex: { kind: "login", login: "codex" },
	cursor: { kind: "login", login: "cursor" },
	github: { kind: "login", login: "github" },
	apns: { kind: "phones", card: "apns" },
};

/**
 * Where each card of a device's page from before went: most to a card of
 * the device's tab, and the few that cover every device to the page.
 */
export const OLD_DEVICE_CARDS: Readonly<Record<string, DeviceCardId | PageCard>> = {
	"new-agents": "new-agents",
	limits: "new-agents",
	placement: "new-agents",
	inbox: "inbox",
	idle: "resources",
	memory: "resources",
	sleep: "resources",
	terminals: "terminals",
	"setup-tokens": "setup-tokens",
	workspaces: "workspaces",
	library: "library",
	network: "network",
	"test-hosts": "test-machines",
	"other-agents": "other-agents",
	cursor: "cursor-usage",
	"app-keys": "app-keys",
};

const isIn = <T extends string>(list: readonly T[], value: string): value is T =>
	(list as readonly string[]).includes(value);

/**
 * The place maxhq://settings/{name} opens: a group, a card, a view of its
 * own or a page from before. Null for maxhq://settings itself, and for a
 * name Settings doesn't know: both open Settings where you left it.
 */
export const settingsPlace = (name: string | null): SettingsPlace | null => {
	if (name === null) return null;
	const old = Object.hasOwn(OLD_SETTINGS_PAGES, name) ? OLD_SETTINGS_PAGES[name] : undefined;
	if (old) return old;
	if (isIn(LOGIN_VIEWS, name)) return { kind: "login", login: name };
	if (isIn(PAGE_CARDS, name) || isIn(SETTINGS_GROUPS, name)) return page(name);
	if (isIn(DEVICE_CARDS, name)) return thisMac(name);
	if (isIn(PHONE_CARDS, name)) return { kind: "phones", card: name };
	return null;
};

/**
 * The place maxhq://device/{device}/{card} opens: the device's tab at the
 * card, or the page's card for one that covers every device. A card
 * Settings doesn't know opens the device's tab at its top.
 */
export const deviceCardPlace = (device: string, card: string): SettingsPlace => {
	const now = Object.hasOwn(OLD_DEVICE_CARDS, card) ? OLD_DEVICE_CARDS[card] : card;
	if (now !== undefined && isIn(DEVICE_CARDS, now)) return { kind: "device", device, card: now };
	if (now !== undefined && isIn(PAGE_CARDS, now)) return page(now);
	return { kind: "device", device, card: null };
};
