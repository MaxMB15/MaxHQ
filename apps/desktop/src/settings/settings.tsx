// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The settings window: the header's gear opens it, and so does Cmd+, or
// Ctrl+, off a Mac. A list on the left picks the page: first the app's own
// appearance, sidebar, editor, agent tools, scheduled tasks, test hosts,
// skills library, notifications, phones and keyboard shortcuts, then the
// logins workspaces use and each project. A field above the list
// narrows it to the pages that have the words you type, in their titles,
// headings or labels. While it does, the devices whose own settings have
// them follow; picking one closes the settings and opens that device's page
// at the card that matched. Every opening starts the forms and the search
// fresh.

import { Dialog } from "@base-ui/react/dialog";
import { Settings, X } from "lucide-react";
import { type CSSProperties, Fragment, type ReactNode, useEffect, useRef, useState } from "react";

import { useServices, useView } from "@/app/services";
import { CopyIcon } from "@/components/copy-field";
import { holdsCommand, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { onMac } from "@/native/platform";
import { SHORTCUT_GROUPS, shortcutRows } from "@/shortcut-list/rows";
import { ShortcutList } from "@/shortcut-list/sheet";
import { PROJECT_SKILLS_TERMS } from "@/skills/skills-tab";
import { SKILLS_LIBRARY_TERMS, SkillsSettingsPage } from "@/skills/library-page";
import type { LinkId } from "@maxhq/shared/bindings";
import { searchWords } from "@maxhq/shared/lib/words";
import { all } from "@maxhq/shared/view/replica";

import { AGENT_TOOLS_TERMS, AgentToolsSettingsPage } from "./agent-tools-settings";
import { APNS_LOGIN_TERMS, ApnsLogin } from "./apns-login";
import { APPEARANCE_TERMS, AppearanceSettingsPage } from "./appearance-settings";
import { CLAUDE_LOGIN_TERMS, ClaudeLogin } from "./claude-login";
import { CODEX_LOGIN_TERMS, CodexLogin } from "./codex-login";
import { FIELD } from "./controls";
import { CURSOR_LOGIN_TERMS, CursorLogin } from "./cursor-login";
import { deviceCards } from "./device-settings";
import { DispatcherSettingsPage } from "./dispatcher-settings";
import { EDITOR_TERMS, EditorSettingsPage } from "./editor-settings";
import { GITHUB_LOGIN_TERMS, GithubLogin } from "./github-login";
import { LOGIN_STEPS } from "./login-steps";
import { NOTIFICATIONS_TERMS, NotificationsSettingsPage } from "./notifications-settings";
import { PHONES_TERMS, PhonesSettingsPage } from "./phones-settings";
import { PROJECT_TERMS, ProjectSettingsPage } from "./project-settings";
import { SCHEDULED_TASKS_TERMS, ScheduledTasksSettingsPage } from "./scheduled-tasks-settings";
import { type Found, findIn, type Span } from "./search";
import { SIDEBAR_TERMS, SidebarSettingsPage } from "./sidebar-settings";
import { TEST_HOSTS_TERMS, TestHostsSettingsPage } from "./test-hosts-settings";
import { UPDATES_TERMS, UpdatesSettingsPage } from "./updates-settings";
import {
	closeSettings,
	type LoginPage,
	openSettings,
	pageKey,
	samePage,
	type SettingsPage,
	settingsOf,
	showDeviceCard,
	showPage,
	useSettings,
} from "./store";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const LOGINS: readonly LoginPage[] = ["claude", "codex", "cursor", "github", "apns"];

const LOGIN_HINT: Record<LoginPage, string> = {
	get claude() {
		return translate("A setup-token from a Mac login, for Claude Code in containers");
	},
	get codex() {
		return translate("Each device's own Codex sign-in, for Codex in containers");
	},
	get cursor() {
		return translate("The Mac's own Cursor login, and an API key for Cursor in containers");
	},
	get github() {
		return translate(
			"The App for pushes from workspaces and # lookups, and the repos it reaches"
		);
	},
	get apns() {
		return translate("The Apple key each device signs its pushes to your phones with");
	},
};

const LOGIN_TERMS: Record<LoginPage, readonly string[]> = {
	claude: CLAUDE_LOGIN_TERMS,
	codex: CODEX_LOGIN_TERMS,
	cursor: CURSOR_LOGIN_TERMS,
	github: GITHUB_LOGIN_TERMS,
	apns: APNS_LOGIN_TERMS,
};

// The dispatcher's page keeps its own file; these are its headings and labels.
const DISPATCHER_TERMS: readonly string[] = ["Account", "Runs as"];

/** A page in the App group. */
type AppPage = {
	readonly kind: Exclude<SettingsPage["kind"], "login" | "project">;
	readonly title: string;
	readonly hint: string;
	/** What the search looks through besides the title and the hint. */
	readonly terms: readonly string[];
	readonly Body: () => ReactNode;
};

const ShortcutsPage = () => {
	useI18n();
	return (
		<div className="rounded-[11px] border border-track bg-card px-4 py-3">
			<ShortcutList />
		</div>
	);
};

// Built when asked for, since the key hints depend on the platform.
const appPages = (): readonly AppPage[] => [
	{
		kind: "appearance",
		title: translate("Appearance"),
		hint: translate("Light, dark or the Mac's own, kept on this Mac"),
		terms: APPEARANCE_TERMS,
		Body: AppearanceSettingsPage,
	},
	{
		kind: "sidebar",
		title: translate("Sidebar"),
		hint: translate("How the sidebar lists agents, kept on this Mac"),
		terms: SIDEBAR_TERMS,
		Body: SidebarSettingsPage,
	},
	{
		kind: "editor",
		title: translate("Editor"),
		hint: translate("Where workspaces and files open, kept on this Mac"),
		terms: EDITOR_TERMS,
		Body: EditorSettingsPage,
	},
	{
		kind: "dispatcher",
		title: translate("Dispatcher"),
		hint: translate("Which account answers the box at the top, kept on this Mac"),
		terms: DISPATCHER_TERMS,
		Body: DispatcherSettingsPage,
	},
	{
		kind: "agent-tools",
		title: translate("Agent tools"),
		hint: translate("What agents and tools may do when they run agents through MaxHQ"),
		terms: AGENT_TOOLS_TERMS,
		Body: AgentToolsSettingsPage,
	},
	{
		kind: "scheduled-tasks",
		title: translate("Scheduled tasks"),
		hint: translate(
			"Prompts each device sends to an agent on a timer, a webhook or GitHub events"
		),
		terms: SCHEDULED_TASKS_TERMS,
		Body: ScheduledTasksSettingsPage,
	},
	{
		kind: "test-hosts",
		title: translate("Test hosts"),
		hint: translate("Which devices run each kind of test machine for the agents there"),
		terms: TEST_HOSTS_TERMS,
		Body: TestHostsSettingsPage,
	},
	{
		kind: "skills",
		title: translate("Skills"),
		hint: translate("The skills projects pick from, and the ones every agent gets"),
		terms: SKILLS_LIBRARY_TERMS,
		Body: SkillsSettingsPage,
	},
	{
		kind: "notifications",
		title: translate("Notifications"),
		hint: translate("Which alerts this Mac shows, and which of them play a sound"),
		terms: NOTIFICATIONS_TERMS,
		Body: NotificationsSettingsPage,
	},
	{
		kind: "updates",
		title: translate("Updates"),
		hint: translate("This app's version, and when it looks for a newer one"),
		terms: UPDATES_TERMS,
		Body: UpdatesSettingsPage,
	},
	{
		kind: "phones",
		title: translate("Phones"),
		hint: translate("The phones that reach your devices, and a code to pair a new one"),
		terms: PHONES_TERMS,
		Body: PhonesSettingsPage,
	},
	{
		kind: "shortcuts",
		title: translate("Keyboard shortcuts"),
		hint: translate("The keys MaxHQ listens for. {{keyHint}} shows them over any view.", {
			keyHint: keyHint(SHORTCUTS.shortcuts),
		}),
		terms: [...SHORTCUT_GROUPS, ...shortcutRows().map((row) => row.what)],
		Body: ShortcutsPage,
	},
];

/** A page the list shows, and what the search found on it. */
type Hit = { readonly key: string; readonly page: SettingsPage; readonly found: Found };

/**
 * A device whose settings the search found, and the card it opens at: none
 * when only the device's name matched.
 */
type DeviceHit = {
	readonly key: string;
	readonly link: LinkId;
	readonly card: string | null;
	readonly found: Found;
};

const Marked = ({ spans }: { spans: readonly Span[] }) => {
	useI18n();
	return spans.map((span, at) =>
		span.hit ? (
			<mark
				key={at}
				className="rounded-[2px] bg-[var(--find-match-bg)] text-[var(--find-match-text)]"
			>
				{span.text}
			</mark>
		) : (
			<Fragment key={at}>{span.text}</Fragment>
		)
	);
};

const NavItem = ({ page, found }: { page: SettingsPage; found: Found }) => {
	useI18n();
	const services = useServices();
	const shown = useSettings((s) => s.page);
	return (
		<Item found={found} on={samePage(page, shown)} onClick={() => showPage(services, page)} />
	);
};

const DeviceItem = ({ hit }: { hit: DeviceHit }) => {
	useI18n();
	const services = useServices();
	return (
		<Item
			found={hit.found}
			on={false}
			onClick={() => showDeviceCard(services, hit.link, hit.card)}
		/>
	);
};

const Item = ({ found, on, onClick }: { found: Found; on: boolean; onClick: () => void }) => {
	useI18n();
	return (
		<button
			type="button"
			aria-current={on ? "page" : undefined}
			onClick={onClick}
			className={cn(
				"flex min-h-[30px] w-full min-w-0 flex-col justify-center rounded-[7px] px-2.5 text-left text-[12.5px] text-secondary-foreground hover:bg-secondary",
				found.term && "py-1",
				on &&
					"bg-selected text-foreground shadow-[inset_0_0_0_1px_var(--selected-ring)] hover:bg-selected"
			)}
		>
			<span className="truncate">
				<Marked spans={found.title} />
			</span>
			{found.term ? (
				<span className="truncate text-[11.5px] text-muted-foreground">
					<Marked spans={found.term} />
				</span>
			) : null}
		</button>
	);
};

const Group = ({ title, children }: { title: string; children: ReactNode }) => {
	useI18n();
	return (
		<div role="group" aria-label={title} className="flex flex-col gap-0.5">
			<span className="px-2.5 pb-1 text-[11px] font-[650] tracking-[0.04em] text-muted-foreground uppercase">
				{title}
			</span>
			{children}
		</div>
	);
};

const PageHeader = ({
	title,
	hint,
	mono,
	copy,
}: {
	title: string;
	hint: string;
	mono?: boolean;
	/** Text the hint's copy icon copies, when it has one. */
	copy?: string;
}) => {
	useI18n();
	return (
		<header className="flex flex-col gap-1">
			<h2 className="text-[18px] font-[650] tracking-[-0.01em] text-foreground-strong">
				{title}
			</h2>
			<p
				className={
					mono
						? "font-mono text-xs text-muted-foreground"
						: "text-[12.5px] text-muted-foreground"
				}
			>
				{hint}
				{copy ? <CopyIcon text={copy} className="-my-1 ml-1 size-[18px]" /> : null}
			</p>
		</header>
	);
};

const Page = ({ page }: { page: SettingsPage }) => {
	useI18n();
	const project = useView((s) =>
		page.kind === "project" ? s.view.tables.project.get(page.project) : undefined
	);
	if (page.kind === "login") {
		return (
			<>
				<PageHeader title={LOGIN_STEPS[page.login].title} hint={LOGIN_HINT[page.login]} />
				{page.login === "claude" ? <ClaudeLogin device={page.device} /> : null}
				{page.login === "codex" ? <CodexLogin /> : null}
				{page.login === "cursor" ? <CursorLogin device={page.device} /> : null}
				{page.login === "github" ? <GithubLogin /> : null}
				{page.login === "apns" ? <ApnsLogin /> : null}
			</>
		);
	}
	if (page.kind === "project") {
		if (!project) {
			return (
				<p className="text-[12.5px] text-muted-foreground">
					{translate("That project is gone.")}
				</p>
			);
		}
		return (
			<>
				<PageHeader
					title={project.name}
					hint={project.repo ?? translate("No remote")}
					mono
					copy={project.repo ?? undefined}
				/>
				<ProjectSettingsPage project={project.id} tab={page.tab} />
			</>
		);
	}
	const app = appPages().find((p) => p.kind === page.kind);
	if (!app) return null;
	return (
		<>
			<PageHeader title={app.title} hint={app.hint} />
			<app.Body />
		</>
	);
};

const Window = () => {
	useI18n();
	const services = useServices();
	const view = useView((s) => s.view);
	const page = useSettings((s) => s.page);
	const opened = useSettings((s) => s.opened);
	const [query, setQuery] = useState("");
	const field = useRef<HTMLInputElement>(null);

	// Command-F finds in the settings while they're open, before the
	// window's own find sees the key.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (!holdsCommand(e) || e.shiftKey || e.altKey || e.key.toLowerCase() !== "f") return;
			e.preventDefault();
			field.current?.focus();
			field.current?.select();
		};
		window.addEventListener("keydown", onKey, true);
		return () => window.removeEventListener("keydown", onKey, true);
	}, []);

	const words = searchWords(query);
	const searching = words.length > 0;
	const hits = (
		key: string,
		target: SettingsPage,
		title: string,
		terms: readonly string[]
	): Hit[] => {
		const found = findIn({ title, terms: terms.map((term) => translate(term)) }, words);
		return found ? [{ key, page: target, found }] : [];
	};
	const logins = LOGINS.flatMap((login) =>
		hits(login, { kind: "login", login }, LOGIN_STEPS[login].title, [
			LOGIN_HINT[login],
			...LOGIN_TERMS[login],
		])
	);
	// A project whose General tab lacks the words but whose Skills tab has
	// them opens on Skills.
	const projects = all(view, "project")
		.sort((a, b) => a.name.localeCompare(b.name))
		.flatMap((p) => {
			const general = hits(p.id, { kind: "project", project: p.id }, p.name, [
				...(p.repo ? [p.repo] : []),
				...PROJECT_TERMS,
			]);
			if (general.length > 0) return general;
			return hits(
				p.id,
				{ kind: "project", project: p.id, tab: "skills" },
				p.name,
				PROJECT_SKILLS_TERMS
			);
		});
	const apps = appPages().flatMap((app) =>
		hits(app.kind, { kind: app.kind }, app.title, [app.hint, ...app.terms])
	);
	// Each device once, at its first card with the words. A device that
	// matches by its name alone opens at the top.
	const seen = new Set<string>();
	const devices: DeviceHit[] = !searching
		? []
		: all(view, "device_link")
				.filter((link) => link.device !== null && !link.claim)
				.sort((a, b) => a.name.localeCompare(b.name))
				.flatMap((link) => {
					const device = link.device!;
					if (seen.has(device)) return [];
					seen.add(device);
					for (const card of deviceCards(view, device)) {
						const found = findIn(
							{ title: link.name, terms: card.terms.map((term) => translate(term)) },
							words
						);
						if (found) {
							const at = found.term === null ? null : card.id;
							return [{ key: link.id, link: link.id, card: at, found }];
						}
					}
					return [];
				});
	const first = [...apps, ...logins, ...projects][0];
	const none = searching && first === undefined && devices.length === 0;

	return (
		<div className="flex h-full min-h-0">
			<nav
				aria-label={translate("Settings pages")}
				className="flex w-[220px] shrink-0 flex-col gap-4 overflow-y-auto border-r border-sidebar-border bg-subpanel px-2.5 py-4"
			>
				<div className="flex flex-col gap-2.5">
					<Dialog.Title className="px-2.5 text-[15px] font-[650] text-foreground-strong">
						{translate("Settings")}
					</Dialog.Title>
					<input
						ref={field}
						type="search"
						aria-label={translate("Search settings")}
						aria-keyshortcuts={onMac() ? "Meta+F" : "Control+F"}
						placeholder={translate("Search")}
						spellCheck={false}
						autoComplete="off"
						value={query}
						onChange={(e) => setQuery(e.target.value)}
						onKeyDown={(e) => {
							// Esc clears the search first, and closes the settings once it's empty.
							if (e.key === "Escape" && query !== "") {
								e.preventDefault();
								e.stopPropagation();
								setQuery("");
							}
							if (e.key === "Enter" && searching && first) {
								e.preventDefault();
								showPage(services, first.page);
							} else if (e.key === "Enter" && searching && devices[0]) {
								e.preventDefault();
								showDeviceCard(services, devices[0].link, devices[0].card);
							}
						}}
						className={FIELD}
					/>
				</div>
				{apps.length > 0 ? (
					<Group title={translate("App")}>
						{apps.map((hit) => (
							<NavItem key={hit.key} page={hit.page} found={hit.found} />
						))}
					</Group>
				) : null}
				{logins.length > 0 ? (
					<Group title={translate("Logins")}>
						{logins.map((hit) => (
							<NavItem key={hit.key} page={hit.page} found={hit.found} />
						))}
					</Group>
				) : null}
				{projects.length > 0 || !searching ? (
					<Group title={translate("Projects")}>
						{projects.length === 0 ? (
							<span className="px-2.5 text-xs text-muted-foreground">
								{translate("No projects yet")}
							</span>
						) : (
							projects.map((hit) => (
								<NavItem key={hit.key} page={hit.page} found={hit.found} />
							))
						)}
					</Group>
				) : null}
				{devices.length > 0 ? (
					<Group title={translate("Devices")}>
						{devices.map((hit) => (
							<DeviceItem key={hit.key} hit={hit} />
						))}
					</Group>
				) : null}
				{none ? (
					<p role="status" className="px-2.5 text-xs text-muted-foreground">
						{translate("No settings match")}
					</p>
				) : null}
			</nav>
			<div
				className="min-w-0 flex-1 overflow-y-auto bg-background px-7 py-6"
				style={{ "--dot-surface": "var(--card)" } as CSSProperties}
			>
				<div
					key={`${opened}:${pageKey(page)}`}
					className="flex max-w-[680px] flex-col gap-[18px]"
				>
					<Page page={page} />
				</div>
			</div>
			<Dialog.Close
				aria-label={translate("Close settings")}
				className="absolute top-3 right-3 grid size-[28px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
			>
				<X aria-hidden size={15} strokeWidth={2} />
			</Dialog.Close>
		</div>
	);
};

/** The header's gear, with the settings window it opens. */
export const SettingsButton = () => {
	useI18n();
	const services = useServices();
	const open = useSettings((s) => s.open);

	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			const command = onMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
			if (e.key !== "," || !command || e.shiftKey || e.altKey || e.repeat) return;
			e.preventDefault();
			if (!settingsOf(services).getState().open) openSettings(services);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [services]);

	return (
		<Dialog.Root
			open={open}
			onOpenChange={(next) => {
				if (!next) closeSettings(services);
			}}
		>
			<button
				type="button"
				aria-label={translate("Settings")}
				aria-keyshortcuts={onMac() ? "Meta+," : "Control+,"}
				title={onMac() ? translate("Settings ⌘,") : translate("Settings Ctrl+,")}
				onClick={() => openSettings(services)}
				className="grid size-[30px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
			>
				<Settings aria-hidden size={17} strokeWidth={1.8} />
			</button>
			<Dialog.Portal>
				<Dialog.Backdrop className="fixed inset-0 z-[120] bg-scrim transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0" />
				<Dialog.Popup
					aria-label={translate("Settings")}
					className="fixed top-1/2 left-1/2 z-[121] h-[min(680px,calc(100vh-64px))] w-[min(920px,calc(100vw-32px))] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-xl border border-input bg-card shadow-popup outline-none"
				>
					<Window />
				</Dialog.Popup>
			</Dialog.Portal>
		</Dialog.Root>
	);
};
