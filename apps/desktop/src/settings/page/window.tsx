// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inside of the Settings window. On the left, a search field and the
// page's seven groups, the one you're in marked as you scroll, then the
// views of their own. On the right, the page: every group under its
// heading, each card with where it applies. A view of its own, such as a
// login or a project, shows in place of the page with Back, and coming
// back finds the page scrolled where it was. While the field has words,
// the page keeps only the cards that have them, and the left column lists
// what matched under its groups.

import { Dialog } from "@base-ui/react/dialog";
import { ArrowLeft, X } from "lucide-react";
import {
	type CSSProperties,
	Fragment,
	memo,
	type ReactNode,
	type RefObject,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
	useSyncExternalStore,
} from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { useServices, useView } from "@/app/services";
import { CopyIcon } from "@/components/copy-field";
import { holdsCommand, keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import { onMac } from "@/native/platform";
import { SkillsSettingsPage } from "@/skills/library-page";
import type { DeviceId, ProjectId } from "@maxhq/shared/bindings";
import type { SettingsDetail, SettingsGroup } from "@maxhq/shared/guide/settings-link";
import { t as translate, useI18n } from "@maxhq/shared/i18n";
import { searchWords } from "@maxhq/shared/lib/words";
import { LOGIN_HINT } from "@maxhq/shared/settings/login-steps";

import { ClaudeLogin } from "../claude-login";
import { CodexLogin } from "../codex-login";
import { CardScope, FIELD, Row, SECONDARY, Section } from "../controls";
import { CursorLogin } from "../cursor-login";
import { GithubLogin } from "../github-login";
import { LocalModelsSettingsPage } from "../local-models-settings";
import { LOGIN_STEPS } from "../login-steps";
import { OpenRouterLogin } from "../openrouter-login";
import { ProjectSettingsPage } from "../project-settings";
import { ScheduledTasksSettingsPage } from "../scheduled-tasks-settings";
import type { Span } from "../search";
import {
	back,
	groupId,
	keepScroll,
	pageCardId,
	revealed,
	scrollOf,
	showPlace,
	useSettings,
} from "../store";
import { buildIndex, GROUPS, ownViews, PAGE, SCOPE_LABEL } from "./cards";
import { DevicesGroup } from "./devices";
import { type Hit, type HitGroup, type Results, searchSettings } from "./find";
import { currentGroup, JUMP_MS } from "./spy";
import { Subheading } from "./subheading";

// The group the left column marks, which the page sets as you scroll.
type Spy = StoreApi<{ readonly at: HitGroup }>;

// Whether scrolls glide, which they don't when you asked for less motion.
const glides = (): boolean =>
	typeof window.matchMedia !== "function" ||
	!window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const scrollBoxTo = (box: HTMLElement, top: number, glide: boolean): void => {
	if (glide && typeof box.scrollTo === "function") box.scrollTo({ top, behavior: "smooth" });
	else box.scrollTop = top;
};

// Where `el` sits in `box`'s scroll, less the room it keeps above itself
// for the group's heading.
const offsetIn = (box: HTMLElement, el: HTMLElement): number =>
	el.getBoundingClientRect().top -
	box.getBoundingClientRect().top +
	box.scrollTop -
	(parseFloat(getComputedStyle(el).scrollMarginTop) || 0);

// The view while you search, and nothing otherwise, so the page doesn't
// render again for every change while you aren't searching.
const useSearchView = (searching: boolean) => {
	const store = useServices().view;
	const get = () => (searching ? store.getState().view : null);
	return useSyncExternalStore(store.subscribe, get, get);
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

const NAV_ITEM =
	"flex min-h-[30px] w-full min-w-0 items-center gap-[9px] rounded-[7px] px-2.5 text-left text-[12.5px] text-secondary-foreground hover:bg-secondary hover:text-foreground";

const GroupButton = ({ group, label, spy }: { group: HitGroup; label: string; spy: Spy }) => {
	useI18n();
	const services = useServices();
	const at = useStore(spy, (s) => s.at);
	const detail = useSettings((s) => s.detail);
	const on = detail === null && at === group;
	return (
		<button
			type="button"
			aria-current={on ? "location" : undefined}
			onClick={() => {
				if (group !== "views") showPlace(services, { kind: "page", at: group });
			}}
			className={cn(NAV_ITEM, on && "bg-selected text-foreground-strong hover:bg-selected")}
		>
			<span
				aria-hidden
				className={cn("h-[14px] w-[3px] shrink-0 rounded-[2px]", on && "bg-primary")}
			/>
			<span className="truncate">{label}</span>
		</button>
	);
};

// Where a hit leads.
const follow = (services: ReturnType<typeof useServices>, hit: Hit): void => {
	switch (hit.kind) {
		case "card":
			return showPlace(services, { kind: "page", at: hit.card });
		case "detail":
			return showPlace(services, hit.place);
		case "device":
			return showPlace(services, { kind: "device", device: hit.device, card: hit.card });
		case "phones":
			return showPlace(services, { kind: "phones", card: hit.card });
	}
};

const HitItem = ({ hit }: { hit: Hit }) => {
	useI18n();
	const services = useServices();
	return (
		<button
			type="button"
			onClick={() => follow(services, hit)}
			className={cn(
				"flex min-h-[28px] w-full min-w-0 flex-col justify-center rounded-[7px] py-0.5 pr-2.5 pl-[22px] text-left text-[12px] text-secondary-foreground hover:bg-secondary",
				hit.found.term && "py-1"
			)}
		>
			<span className="truncate">
				<Marked spans={hit.found.title} />
			</span>
			{hit.found.term ? (
				<span className="truncate text-[11px] text-muted-foreground">
					<Marked spans={hit.found.term} />
				</span>
			) : null}
		</button>
	);
};

const ViewsLabel = () => {
	useI18n();
	return (
		<span className="mx-2.5 mt-4 mb-1 text-[11px] font-[600] tracking-[0.04em] text-muted-foreground uppercase">
			{translate("Their own views")}
		</span>
	);
};

// The views of their own, which leave the page for a view with Back, and
// the keyboard shortcuts, which show over any view.
const OwnViewLinks = () => {
	useI18n();
	const services = useServices();
	const detail = useSettings((s) => s.detail);
	return (
		<div role="group" aria-label={translate("Their own views")} className="flex flex-col">
			<ViewsLabel />
			{ownViews().map((own) => {
				const on = detail?.kind === own.kind;
				return (
					<button
						key={own.kind}
						type="button"
						aria-current={on ? "page" : undefined}
						aria-keyshortcuts={
							own.kind === "shortcuts"
								? onMac()
									? "Meta+/"
									: "Control+/"
								: undefined
						}
						onClick={() => showPlace(services, { kind: own.kind })}
						className={cn(
							"flex min-h-[28px] w-full min-w-0 items-center gap-2 rounded-[7px] px-2.5 text-left text-[12.5px] text-secondary-foreground hover:bg-secondary hover:text-foreground",
							on && "bg-selected text-foreground-strong hover:bg-selected"
						)}
					>
						<span className="min-w-0 flex-1 truncate">{own.title}</span>
						{own.kind === "shortcuts" ? (
							<kbd className="rounded-[4px] border border-input px-[5px] font-mono text-[11px] text-secondary-foreground">
								{keyHint(SHORTCUTS.shortcuts)}
							</kbd>
						) : (
							<span aria-hidden className="text-muted-foreground">
								↗
							</span>
						)}
					</button>
				);
			})}
		</div>
	);
};

const Nav = ({
	results,
	spy,
	field,
	query,
	onQuery,
}: {
	results: Results | null;
	spy: Spy;
	field: RefObject<HTMLInputElement | null>;
	query: string;
	onQuery: (query: string) => void;
}) => {
	useI18n();
	const services = useServices();
	const views = results?.hits.filter((hit) => hit.group === "views") ?? [];
	return (
		<nav
			aria-label={translate("Settings")}
			className="flex w-[228px] shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-sidebar-border bg-subpanel px-2.5 py-3.5"
		>
			<Dialog.Title className="sr-only">{translate("Settings")}</Dialog.Title>
			<input
				ref={field}
				type="search"
				aria-label={translate("Search settings")}
				aria-keyshortcuts={onMac() ? "Meta+F" : "Control+F"}
				placeholder={translate("Search settings")}
				spellCheck={false}
				autoComplete="off"
				value={query}
				onChange={(e) => onQuery(e.target.value)}
				onKeyDown={(e) => {
					// Esc clears the search first, and leaves it to the window once it's empty.
					if (e.key === "Escape" && query !== "") {
						e.preventDefault();
						e.stopPropagation();
						onQuery("");
					}
					const first = results?.hits[0];
					if (e.key === "Enter" && first) {
						e.preventDefault();
						follow(services, first);
					}
				}}
				className={cn(FIELD, "mb-2 h-[28px] w-full")}
			/>
			{GROUPS.filter(({ group }) => !results || results.groups.has(group)).map(
				({ group, label }) => (
					<Fragment key={group}>
						<GroupButton group={group} label={translate(label)} spy={spy} />
						{results?.hits
							.filter((hit) => hit.group === group)
							.map((hit) => (
								<HitItem key={hit.key} hit={hit} />
							))}
					</Fragment>
				)
			)}
			{!results ? <OwnViewLinks /> : null}
			{views.length > 0 ? (
				<div
					role="group"
					aria-label={translate("Their own views")}
					className="flex flex-col"
				>
					<ViewsLabel />
					{views.map((hit) => (
						<HitItem key={hit.key} hit={hit} />
					))}
				</div>
			) : null}
			{results && results.hits.length === 0 ? (
				<p role="status" className="px-2.5 text-xs text-muted-foreground">
					{translate("No settings match")}
				</p>
			) : null}
		</nav>
	);
};

const GroupSection = ({
	group,
	label,
	hint,
	children,
}: {
	group: HitGroup;
	label: string;
	hint: string;
	children: ReactNode;
}) => {
	useI18n();
	return (
		<section
			id={group === "views" ? undefined : groupId(group)}
			data-group={group}
			aria-label={label}
			className="flex flex-col gap-3 pt-2.5"
		>
			<div className="sticky top-0 z-[2] -mx-1 flex flex-wrap items-baseline gap-x-2.5 bg-linear-to-b from-background from-[78%] to-transparent px-1 pt-3.5 pb-2.5">
				<h2 className="text-[16px] font-[600] text-foreground-strong">{label}</h2>
				<p className="text-[12px] text-muted-foreground">{hint}</p>
			</div>
			{children}
		</section>
	);
};

// A group's cards, each under the small heading it sits under, if any.
const GroupCards = ({ group, results }: { group: SettingsGroup; results: Results | null }) => {
	useI18n();
	const cards = PAGE.filter(
		(entry) => entry.group === group && (!results || results.cards.has(entry.card))
	);
	return cards.map((entry, at) => (
		<Fragment key={entry.card}>
			{entry.under && entry.under !== cards[at - 1]?.under ? (
				<Subheading title={translate(entry.under)} />
			) : null}
			<div id={pageCardId(entry.card)} className="scroll-mt-[60px]">
				<CardScope.Provider value={translate(SCOPE_LABEL[entry.scope])}>
					<entry.Body results={results} />
				</CardScope.Provider>
			</div>
		</Fragment>
	));
};

// The views of their own a search found, for the page to lead to as well.
const ViewHits = ({ hits }: { hits: readonly Hit[] }) => {
	useI18n();
	const services = useServices();
	return (
		<Section title={translate("Their own views")}>
			{hits.map((hit) => (
				<Row key={hit.key} title={<Marked spans={hit.found.title} />}>
					<button
						type="button"
						className={SECONDARY}
						onClick={() => follow(services, hit)}
					>
						{translate("Open")}
					</button>
				</Row>
			))}
		</Section>
	);
};

// The page, every group under its heading. It sets the left column's mark
// as you scroll, and scrolls to what a link or a click asks for.
const Page = memo(function Page({
	results,
	searched,
	spy,
}: {
	results: Results | null;
	/** The words searched for, so a new search starts at the top. */
	searched: string;
	spy: Spy;
}) {
	useI18n();
	const services = useServices();
	const reveal = useSettings((s) => s.reveal);
	const box = useRef<HTMLDivElement>(null);
	const first = useRef(true);
	// When the page last scrolled to what was asked for, so the left
	// column keeps the group it marked while the scroll glides there.
	const jumped = useRef(0);
	const lastSearch = useRef(searched);

	useLayoutEffect(() => {
		const scroller = box.current;
		if (!scroller) return;
		const mounting = first.current;
		first.current = false;
		if (reveal === null) {
			if (mounting) scroller.scrollTop = scrollOf(services);
			return;
		}
		const el = document.getElementById(reveal);
		if (el && scroller.contains(el)) {
			const group = el.closest("[data-group]")?.getAttribute("data-group");
			if (group) spy.setState({ at: group as HitGroup });
			jumped.current = Date.now();
			scrollBoxTo(scroller, offsetIn(scroller, el), !mounting && glides());
		}
		revealed(services);
	}, [reveal, services, spy]);

	useLayoutEffect(() => {
		if (lastSearch.current === searched || !box.current) return;
		lastSearch.current = searched;
		box.current.scrollTop = 0;
	}, [searched]);

	const onScroll = () => {
		const scroller = box.current;
		if (!scroller) return;
		keepScroll(services, scroller.scrollTop);
		if (Date.now() - jumped.current < JUMP_MS) return;
		const top = scroller.getBoundingClientRect().top;
		const marks = [...scroller.querySelectorAll<HTMLElement>("[data-group]")];
		const at = currentGroup(
			marks.map((mark) => mark.getBoundingClientRect().top - top),
			{
				top: scroller.scrollTop,
				height: scroller.clientHeight,
				scrollHeight: scroller.scrollHeight,
			}
		);
		const group = marks[at]?.getAttribute("data-group");
		if (group && group !== spy.getState().at) spy.setState({ at: group as HitGroup });
	};

	const views = results?.hits.filter((hit) => hit.group === "views") ?? [];
	return (
		<div
			ref={box}
			onScroll={onScroll}
			className="min-w-0 flex-1 overflow-y-auto bg-background px-8 pb-10"
			style={{ "--dot-surface": "var(--card)" } as CSSProperties}
		>
			<div className="flex max-w-[700px] flex-col gap-3">
				{GROUPS.filter(({ group }) => !results || results.groups.has(group)).map(
					({ group, label, hint }) => (
						<GroupSection
							key={group}
							group={group}
							label={translate(label)}
							hint={translate(hint)}
						>
							{group === "devices" ? (
								<DevicesGroup results={results} />
							) : (
								<GroupCards group={group} results={results} />
							)}
						</GroupSection>
					)
				)}
				{views.length > 0 ? (
					<GroupSection
						group="views"
						label={translate("Their own views")}
						hint={translate("Views that open in place of the page")}
					>
						<ViewHits hits={views} />
					</GroupSection>
				) : null}
			</div>
		</div>
	);
});

const DetailHeader = ({
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
				{mono ? <span className="selectable">{hint}</span> : hint}
				{copy ? <CopyIcon text={copy} className="-my-1 ml-1 size-[18px]" /> : null}
			</p>
		</header>
	);
};

const ProjectDetail = ({ id, tab }: { id: ProjectId; tab?: "general" | "skills" }) => {
	useI18n();
	const project = useView((s) => s.view.tables.project.get(id));
	if (!project) {
		return (
			<p className="text-[12.5px] text-muted-foreground">
				{translate("That project is gone.")}
			</p>
		);
	}
	return (
		<>
			<DetailHeader
				title={project.name}
				hint={project.repo ?? translate("No remote")}
				mono
				copy={project.repo ?? undefined}
			/>
			<ProjectSettingsPage project={project.id} tab={tab} />
		</>
	);
};

const DetailBody = ({ detail }: { detail: SettingsDetail }) => {
	useI18n();
	if (detail.kind === "login") {
		const device = detail.device as DeviceId | undefined;
		return (
			<>
				<DetailHeader
					title={LOGIN_STEPS[detail.login].title}
					hint={LOGIN_HINT[detail.login]}
				/>
				{detail.login === "claude" ? <ClaudeLogin device={device} /> : null}
				{detail.login === "codex" ? <CodexLogin /> : null}
				{detail.login === "cursor" ? <CursorLogin device={device} /> : null}
				{detail.login === "openrouter" ? <OpenRouterLogin device={device} /> : null}
				{detail.login === "github" ? <GithubLogin /> : null}
			</>
		);
	}
	if (detail.kind === "project") {
		return <ProjectDetail id={detail.project as ProjectId} tab={detail.tab} />;
	}
	const own = ownViews().find((view) => view.kind === detail.kind);
	return (
		<>
			{own ? <DetailHeader title={own.title} hint={own.hint} /> : null}
			{detail.kind === "skills" ? <SkillsSettingsPage /> : null}
			{detail.kind === "scheduled-tasks" ? <ScheduledTasksSettingsPage /> : null}
			{detail.kind === "local-models" ? <LocalModelsSettingsPage /> : null}
		</>
	);
};

// A view of its own, in place of the page, with Back.
const Detail = ({ detail }: { detail: SettingsDetail }) => {
	useI18n();
	const services = useServices();
	return (
		<div
			className="min-w-0 flex-1 overflow-y-auto bg-background px-8 pt-4 pb-10"
			style={{ "--dot-surface": "var(--card)" } as CSSProperties}
		>
			<div className="flex max-w-[700px] flex-col gap-[18px]">
				<button
					type="button"
					aria-keyshortcuts="Escape"
					onClick={() => back(services)}
					className="-ml-2 flex h-[26px] w-fit items-center gap-1.5 rounded-[6px] px-2 text-[12.5px] text-secondary-foreground hover:bg-secondary hover:text-foreground"
				>
					<ArrowLeft aria-hidden size={14} strokeWidth={2} />
					{translate("Back")}
				</button>
				<DetailBody detail={detail} />
			</div>
		</div>
	);
};

/** What the settings window shows. */
export const SettingsWindow = () => {
	const { language } = useI18n();
	const services = useServices();
	const detail = useSettings((s) => s.detail);
	const opened = useSettings((s) => s.opened);
	const [query, setQuery] = useState("");
	const field = useRef<HTMLInputElement>(null);
	const [spy] = useState<Spy>(() => createStore(() => ({ at: "general" as HitGroup })));

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
	const view = useSearchView(words.length > 0);
	const searched = words.join(" ");
	const results = useMemo(
		() => (view ? searchSettings(buildIndex(view), searched.split(" ")) : null),
		// The words come in through `searched`, and the labels change with the language.
		// eslint-disable-next-line react-hooks/exhaustive-deps
		[view, searched, language]
	);

	return (
		<div className="flex h-full min-h-0">
			<Nav
				results={results}
				spy={spy}
				field={field}
				query={query}
				onQuery={(next) => {
					setQuery(next);
					// Words typed over a view of its own go back to the page, where they show.
					if (searchWords(next).length > 0 && detail) back(services);
				}}
			/>
			{detail ? (
				<Detail key={`${opened}:${JSON.stringify(detail)}`} detail={detail} />
			) : (
				<Page key={opened} results={results} searched={searched} spy={spy} />
			)}
			<Dialog.Close
				aria-label={translate("Close settings")}
				className="absolute top-3 right-3 z-[3] grid size-[28px] place-items-center rounded-[7px] text-secondary-foreground hover:bg-secondary"
			>
				<X aria-hidden size={15} strokeWidth={2} />
			</Dialog.Close>
		</div>
	);
};
