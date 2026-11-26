// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the Settings page holds, in order: its seven groups, each card with
// the group it sits in, where it applies and the labels the search looks
// through, the views of their own, and the index a search runs over. The
// guide's map reads the same lists, so a card the page shows is a card
// the guide covers.

import type { ComponentType } from "react";

import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { SHORTCUT_GROUPS, shortcutRows } from "@/shortcut-list/rows";
import { SKILLS_LIBRARY_TERMS } from "@/skills/library-page";
import { PROJECT_SKILLS_TERMS } from "@/skills/skills-tab";
import type { PageCard, SettingsGroup } from "@maxhq/shared/guide/settings-link";
import { LOGIN_VIEWS } from "@maxhq/shared/guide/settings-link";
import { t as translate } from "@maxhq/shared/i18n";
import { accountDevices } from "@maxhq/shared/settings/accounts";
import { LOGIN_HINT, type LoginPage } from "@maxhq/shared/settings/login-steps";
import { all, type ViewState } from "@maxhq/shared/view/replica";

import {
	ACCESS_KEYS_TERMS,
	AccessKeysCard,
	GRANTS_TERMS,
	GrantsCard,
	NEW_PERMISSIONS_TERMS,
	NewPermissionsCard,
} from "../agent-tools-settings";
import { APNS_LOGIN_TERMS } from "../apns-login";
import { APP_KEYS_TERMS, AppKeysCard } from "../app-keys";
import { APPEARANCE_TERMS, AppearanceSettingsPage } from "../appearance-settings";
import { CLAUDE_LOGIN_TERMS } from "../claude-login";
import { CODEX_LOGIN_TERMS } from "../codex-login";
import { CURSOR_LOGIN_TERMS } from "../cursor-login";
import {
	CLAUDE_ARTIFACTS_TERMS,
	ClaudeArtifactsCard,
	CURSOR_USAGE_TERMS,
	CursorUsageCard,
	deviceCards,
	OTHER_AGENTS_TERMS,
	OtherAgentsCard,
} from "../device-settings";
import { DISPATCHER_TERMS, DispatcherSettingsPage } from "../dispatcher-settings";
import { DOCK_BADGE_TERMS, DockBadgeSection } from "../dock-badge-settings";
import {
	COMPOSER_TERMS,
	ComposerSettings,
	OPEN_IN_TERMS,
	OpenInSettings,
} from "../editor-settings";
import { GITHUB_LOGIN_TERMS } from "../github-login";
import { LOCAL_MODELS_TERMS } from "../local-models-settings";
import { LOGIN_STEPS } from "../login-steps";
import { OPENROUTER_LOGIN_TERMS } from "../openrouter-login";
import { NOTIFICATIONS_TERMS, NotificationsSettingsPage } from "../notifications-settings";
import { PHONES_TERMS } from "../phones-settings";
import { PORT_PROMPTS_TERMS, PortPromptsSection } from "../port-prompts-settings";
import { PROJECT_TERMS } from "../project-settings";
import { SCHEDULED_TASKS_TERMS } from "../scheduled-tasks-settings";
import { SIDEBAR_TERMS, SidebarSettingsPage } from "../sidebar-settings";
import { UPDATES_TERMS, UpdatesSettingsPage } from "../updates-settings";
import { ACCOUNTS_TERMS, AccountsCard } from "./accounts";
import type { DetailEntry, Results, SearchIndex } from "./find";
import { PROJECTS_TERMS, ProjectsCard } from "./projects";
import { REMEMBERED_TERMS, RememberedAnswersCard } from "./remembered";

/** A group of the page: its heading and the line under it, in English, as their keys. */
export type GroupEntry = {
	readonly group: SettingsGroup;
	readonly label: string;
	readonly hint: string;
};

export const GROUPS: readonly GroupEntry[] = [
	{ group: "general", label: "General", hint: "Look, language and updates" },
	{ group: "chat", label: "Chat and agents", hint: "The composer, the sidebar, the dispatcher" },
	{ group: "notifications", label: "Notifications", hint: "What MaxHQ tells you, and where" },
	{ group: "accounts", label: "Accounts", hint: "Who agents run as, on each device" },
	{ group: "devices", label: "Devices and phones", hint: "Each device's own settings" },
	{
		group: "permissions",
		label: "Permissions",
		hint: "What agents may do to each other and to devices",
	},
	{ group: "projects", label: "Projects", hint: "Each project's own settings" },
];

/** Where a card applies: on this Mac alone, or on every device. */
export type Scope = "mac" | "every";

/** The tag a card's scope shows, as its key. */
export const SCOPE_LABEL: Record<Scope, string> = { mac: "This Mac", every: "Every device" };

/** A card of the page, outside the device tabs. */
export type PageEntry = {
	readonly card: PageCard;
	readonly group: SettingsGroup;
	/** Its title, in English, as its key. */
	readonly title: string;
	readonly scope: Scope;
	/** The labels on it, as their keys, which the search and the guide look through. */
	readonly terms: readonly string[];
	/** The small heading it sits under in its group, if any, as its key. */
	readonly under?: string;
	readonly Body: ComponentType<{ readonly results: Results | null }>;
};

/** The page's cards, in order. */
export const PAGE: readonly PageEntry[] = [
	{
		card: "appearance",
		group: "general",
		title: "Appearance",
		scope: "mac",
		terms: APPEARANCE_TERMS,
		Body: AppearanceSettingsPage,
	},
	{
		card: "updates",
		group: "general",
		title: "Updates",
		scope: "mac",
		terms: UPDATES_TERMS,
		Body: UpdatesSettingsPage,
	},
	{
		card: "composer",
		group: "chat",
		title: "Composer",
		scope: "mac",
		terms: COMPOSER_TERMS,
		Body: ComposerSettings,
	},
	{
		card: "open-in",
		group: "chat",
		title: "Open in",
		scope: "mac",
		terms: OPEN_IN_TERMS,
		Body: OpenInSettings,
	},
	{
		card: "above-conversation",
		group: "chat",
		title: "Above the conversation",
		scope: "mac",
		terms: PORT_PROMPTS_TERMS,
		Body: PortPromptsSection,
	},
	{
		card: "sidebar",
		group: "chat",
		title: "Sidebar",
		scope: "mac",
		terms: SIDEBAR_TERMS,
		Body: SidebarSettingsPage,
	},
	{
		card: "dispatcher",
		group: "chat",
		title: "Dispatcher",
		scope: "mac",
		terms: DISPATCHER_TERMS,
		Body: DispatcherSettingsPage,
	},
	{
		card: "notifications",
		group: "notifications",
		title: "Notifications",
		scope: "mac",
		terms: NOTIFICATIONS_TERMS,
		Body: NotificationsSettingsPage,
	},
	{
		card: "badge",
		group: "notifications",
		title: "Badge",
		scope: "mac",
		terms: DOCK_BADGE_TERMS,
		Body: DockBadgeSection,
	},
	{
		card: "accounts",
		group: "accounts",
		title: "Logins",
		scope: "every",
		terms: ACCOUNTS_TERMS,
		Body: AccountsCard,
	},
	{
		card: "cursor-usage",
		group: "accounts",
		title: "Cursor usage",
		scope: "every",
		terms: CURSOR_USAGE_TERMS,
		Body: CursorUsageCard,
	},
	{
		card: "other-agents",
		group: "permissions",
		title: "Other agents",
		scope: "every",
		terms: OTHER_AGENTS_TERMS,
		under: "Between agents",
		Body: OtherAgentsCard,
	},
	{
		card: "grants",
		group: "permissions",
		title: "Grants",
		scope: "every",
		terms: GRANTS_TERMS,
		under: "Between agents",
		Body: GrantsCard,
	},
	{
		card: "new-permissions",
		group: "permissions",
		title: "New permissions",
		scope: "every",
		terms: NEW_PERMISSIONS_TERMS,
		under: "Between agents",
		Body: NewPermissionsCard,
	},
	{
		card: "claude-artifacts",
		group: "permissions",
		title: "claude.ai pages",
		scope: "every",
		terms: CLAUDE_ARTIFACTS_TERMS,
		under: "Outside tools",
		Body: ClaudeArtifactsCard,
	},
	{
		card: "access-keys",
		group: "permissions",
		title: "Access keys",
		scope: "every",
		terms: ACCESS_KEYS_TERMS,
		under: "Outside tools",
		Body: AccessKeysCard,
	},
	{
		card: "app-keys",
		group: "permissions",
		title: "App keys",
		scope: "every",
		terms: APP_KEYS_TERMS,
		under: "Outside tools",
		Body: AppKeysCard,
	},
	{
		card: "remembered-answers",
		group: "permissions",
		title: "Remembered answers",
		scope: "mac",
		terms: REMEMBERED_TERMS,
		under: "Outside tools",
		Body: RememberedAnswersCard,
	},
	{
		card: "projects",
		group: "projects",
		title: "Projects",
		scope: "every",
		terms: PROJECTS_TERMS,
		Body: ProjectsCard,
	},
];

/** A view of its own the left column lists, or the keyboard shortcuts. */
export type OwnView = {
	readonly kind: "skills" | "scheduled-tasks" | "local-models" | "shortcuts";
	readonly title: string;
	readonly hint: string;
	/** As their keys. */
	readonly terms: readonly string[];
};

// Built when asked for, since the key hints depend on the platform.
/** The views of their own, in the left column's order, their titles and hints translated. */
export const ownViews = (): readonly OwnView[] => [
	{
		kind: "skills",
		title: translate("Skills"),
		hint: translate("The skills projects pick from, and the ones every agent gets"),
		terms: SKILLS_LIBRARY_TERMS,
	},
	{
		kind: "scheduled-tasks",
		title: translate("Scheduled tasks"),
		hint: translate(
			"Prompts each device sends to an agent on a timer, a webhook or GitHub events"
		),
		terms: SCHEDULED_TASKS_TERMS,
	},
	{
		kind: "local-models",
		title: translate("Local models"),
		hint: translate(
			"Models each device runs with llama.cpp, and servers such as Ollama it reaches by URL"
		),
		terms: LOCAL_MODELS_TERMS,
	},
	{
		kind: "shortcuts",
		title: translate("Keyboard shortcuts"),
		hint: translate("The keys MaxHQ listens for. {{keyHint}} shows them over any view.", {
			keyHint: keyHint(SHORTCUTS.shortcuts),
		}),
		terms: [...SHORTCUT_GROUPS, ...shortcutRows().map((row) => row.what)],
	},
];

/** Each login's labels, as their keys, for the search and the guide. */
export const LOGIN_TERMS: Record<LoginPage, readonly string[]> = {
	claude: CLAUDE_LOGIN_TERMS,
	codex: CODEX_LOGIN_TERMS,
	cursor: CURSOR_LOGIN_TERMS,
	openrouter: OPENROUTER_LOGIN_TERMS,
	github: GITHUB_LOGIN_TERMS,
	apns: APNS_LOGIN_TERMS,
};

const translated = (terms: readonly string[]): string[] => terms.map((term) => translate(term));

/** Everything a search of Settings looks through, translated, from what `view` holds. */
export const buildIndex = (view: ViewState): SearchIndex => {
	const logins: DetailEntry[] = LOGIN_VIEWS.map((login) => ({
		key: `login-${login}`,
		group: "accounts",
		place: { kind: "login", login },
		title: LOGIN_STEPS[login].title,
		terms: [LOGIN_HINT[login], ...translated(LOGIN_TERMS[login])],
	}));
	// A project whose General tab lacks the words but whose Skills tab has
	// them opens on Skills: both share the project's key, General first.
	const projects: DetailEntry[] = all(view, "project")
		.sort((a, b) => a.name.localeCompare(b.name))
		.flatMap((project) => [
			{
				key: project.id,
				group: "projects" as const,
				place: { kind: "project" as const, project: project.id },
				title: project.name,
				terms: [...(project.repo ? [project.repo] : []), ...translated(PROJECT_TERMS)],
			},
			{
				key: project.id,
				group: "projects" as const,
				place: { kind: "project" as const, project: project.id, tab: "skills" as const },
				title: project.name,
				terms: translated(PROJECT_SKILLS_TERMS),
			},
		]);
	const views: DetailEntry[] = ownViews().map((own) => ({
		key: own.kind,
		group: "views",
		place: { kind: own.kind },
		title: own.title,
		terms: [own.hint, ...translated(own.terms)],
	}));
	return {
		groups: GROUPS.map(({ group, label }) => ({ group, label: translate(label) })),
		cards: PAGE.map((entry) => ({
			card: entry.card,
			group: entry.group,
			title: translate(entry.title),
			terms: translated(entry.terms),
		})),
		details: [...logins, ...projects, ...views],
		devices: accountDevices(view).map(({ device, name }) => ({
			device,
			name,
			cards: deviceCards(view, device),
		})),
		phones: [
			{ card: "phones", title: translate("Phones"), terms: translated(PHONES_TERMS) },
			{
				card: "apns",
				title: LOGIN_STEPS.apns.title,
				terms: [LOGIN_HINT.apns, ...translated(APNS_LOGIN_TERMS)],
			},
		],
	};
};
