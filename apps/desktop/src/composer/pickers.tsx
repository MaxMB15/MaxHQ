// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The footer's model and permission pickers. The labels are the same for
// every vendor, and a change applies from the agent's next message. A
// model of another vendor moves the agent there. The agent view's header
// draws the model picker too, as a smaller chip that opens downward. In a
// narrow footer the permission picker folds to its icon and the model's
// name gets cut short, and their tooltips and accessible names keep the
// full text. A long list of models, such as an OpenRouter login's, gets a
// search field at the top of its menu.

import { Menu } from "@base-ui/react/menu";
import {
	Asterisk,
	Bot,
	Check,
	ChevronDown,
	Cpu,
	Hexagon,
	type LucideIcon,
	MousePointer2,
	Settings2,
	Shield,
} from "lucide-react";
import { Fragment, type ReactNode, useState } from "react";

import {
	currentModel,
	modelLabel,
	moveModels,
	NOT_OFFERED,
	permissionChoices,
	permissionLabel,
	permissionNote,
	runningMode,
} from "@maxhq/shared/agent-view/controls";
import { filterModels, showsModelSearch } from "@maxhq/shared/agent-view/model-search";
import type {
	Account,
	AccountId,
	AcpFeatures,
	AgentPermission,
	ModelChoice,
	PermissionMode,
	Vendor,
} from "@maxhq/shared/bindings";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { TONE } from "@/tasks/activity";
import type { ModelTag } from "@maxhq/shared/local-models/picker";
import { accountKind, type AgentKind } from "@maxhq/shared/status/agent-kind";
import { accountName } from "@maxhq/shared/status/usage";
import type { Move } from "@maxhq/shared/view/select";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

const MODEL_ICON: Record<AgentKind, LucideIcon> = {
	claude: Asterisk,
	codex: Hexagon,
	cursor: MousePointer2,
	local: Cpu,
	acp: Bot,
};

/** What the model picker shows for local models. `useLocalMenu` makes one. */
export type LocalMenu = {
	/** What a local account's models are listed under, such as "This Mac". */
	readonly group: (account: Account) => string;
	/** The tag beside a local account's model, such as a green "Fits", or null. */
	readonly tag: (account: Account, model: string) => ModelTag | null;
	/** Opens the Local models settings. */
	readonly onSetup: () => void;
};

/** A model's tag in a menu, such as a green "Fits". */
const TagPill = ({ tag }: { tag: ModelTag | null }) =>
	tag && (
		<span
			className={cn(
				"h-[18px] shrink-0 rounded-[4px] px-1.5 text-[10.5px] leading-[18px] font-[600]",
				TONE[tag.tone]
			)}
		>
			{tag.label}
		</span>
	);

/** The look of a footer picker's button. */
export const TRIGGER =
	"inline-flex h-[26px] shrink-0 items-center gap-[5px] rounded-md px-2 text-xs whitespace-nowrap text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-border data-popup-open:text-foreground";

const HEADER_TRIGGER =
	"inline-flex h-[22px] shrink-0 items-center gap-1 rounded-md px-1.5 text-[11.5px] whitespace-nowrap text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-border data-popup-open:text-foreground";

/** The look of a menu's popup, shared with the header's other menus. */
export const MENU_POPUP =
	"w-[300px] rounded-[10px] border border-input bg-popover p-[5px] text-popover-foreground shadow-menu outline-none";

/** The look of one item in a menu. */
export const MENU_ITEM =
	"flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5 outline-none select-none data-checked:bg-selected data-disabled:opacity-50 data-highlighted:bg-accent";

/** Where a picker sits: the composer's footer, or the agent view's header. */
export type PickerPlace = "footer" | "header";

/**
 * How a chip in the footer makes room: down to its icon, or by cutting its
 * label short, which it then draws in a span that `truncate`s and keeps
 * a few letters wide.
 */
export type PickerFold = "icon" | "shrink";

// A chip folded to its icon is as wide as it is tall. One cut short can
// shrink down to its icon, four letters and its chevron: 16 pixels of
// padding, 12 of icon, two gaps of 5 and 9 of chevron. Below that the row
// runs past its edge, so it folds its next step.
export const FOLDED: Record<PickerFold, string> = {
	icon: "w-[26px] justify-center px-0",
	shrink: "min-w-[calc(4ch+47px)] shrink",
};

// The model's name cut short where it can't be changed, without the chevron.
const CUT_PLAIN = "min-w-[calc(4ch+33px)] shrink";

/**
 * A search field at the top of a picker's menu, which narrows its items as
 * you type. The menu filters nothing itself: the picker leaves out what
 * doesn't match. Closing the menu empties the field.
 */
export type PickerSearch = {
	readonly query: string;
	readonly onQuery: (query: string) => void;
	/** The field's name, which it shows until you type. */
	readonly label: string;
	/** What shows under the field when nothing matches, or null. */
	readonly empty: ReactNode;
};

const SEARCH_FIELD =
	"mb-[5px] h-7 w-full shrink-0 rounded-[6px] border border-input bg-card px-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground focus:border-ring";

export const Picker = ({
	label,
	trigger,
	place = "footer",
	fold = null,
	tip = null,
	search = null,
	children,
}: {
	/** Its accessible name, which says what the trigger shows in full. */
	label: string;
	trigger: ReactNode;
	place?: PickerPlace;
	fold?: PickerFold | null;
	/** What its tooltip says, or null for none. */
	tip?: ReactNode;
	/** A search field at the top of its menu, or null for none. */
	search?: PickerSearch | null;
	children: ReactNode;
}) => {
	useI18n();
	const menu = (
		<Menu.Root>
			<Tooltip content={tip}>
				<Menu.Trigger
					className={cn(
						place === "header" ? HEADER_TRIGGER : TRIGGER,
						fold && FOLDED[fold]
					)}
					aria-label={label}
				>
					{trigger}
					{fold !== "icon" && (
						<ChevronDown
							aria-hidden
							className="size-[9px] shrink-0"
							strokeWidth={2.6}
						/>
					)}
				</Menu.Trigger>
			</Tooltip>
			<Menu.Portal>
				<Menu.Positioner
					side={place === "header" ? "bottom" : "top"}
					align="start"
					sideOffset={6}
					className="z-[130]"
				>
					{search ? (
						// The menu keeps its height while you type, so the field
						// stays put above a list that opens upward.
						<Menu.Popup
							className={cn(
								MENU_POPUP,
								"flex h-[min(420px,var(--available-height))] flex-col"
							)}
						>
							<Menu.Input
								aria-label={search.label}
								placeholder={search.label}
								spellCheck={false}
								autoComplete="off"
								className={SEARCH_FIELD}
							/>
							{search.empty}
							<Menu.List className="min-h-0 flex-1 overflow-y-auto">
								{children}
							</Menu.List>
						</Menu.Popup>
					) : (
						<Menu.Popup
							className={cn(
								MENU_POPUP,
								"max-h-[min(420px,var(--available-height))] overflow-y-auto"
							)}
						>
							{children}
						</Menu.Popup>
					)}
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
	// Base UI's filtering takes the keys a plain menu would use to jump to an
	// item as you type, and moves the highlight with the arrows while the
	// field keeps focus. `filter={null}` leaves the matching to the picker.
	return search ? (
		<Menu.FilterProvider filter={null} value={search.query} onValueChange={search.onQuery}>
			{menu}
		</Menu.FilterProvider>
	) : (
		menu
	);
};

export const Tick = () => {
	useI18n();
	return (
		<span className="flex size-3.5 items-center justify-center">
			<Menu.RadioItemIndicator>
				<Check aria-hidden className="size-3.5 text-primary" strokeWidth={2.6} />
			</Menu.RadioItemIndicator>
		</span>
	);
};

export const GROUP_LABEL = "px-2 pt-[5px] pb-1 text-[11px] text-muted-foreground";

/**
 * A model under another account, which moves the agent there. A local
 * account's models name their server, such as "Move to Local, This Mac".
 */
const MoveGroup = ({
	move,
	models,
	local,
	onPick,
}: {
	move: Move;
	/** The models it lists, `moveModels` or those a search left. */
	models: readonly Pick<ModelChoice, "id" | "label">[];
	local: LocalMenu | undefined;
	onPick: (model: string, account: AccountId) => void;
}) => {
	useI18n();
	const kind = accountKind(move.account);
	const Icon = MODEL_ICON[kind];
	const server = kind === "local" ? local?.group(move.account) : undefined;
	return (
		<Menu.Group>
			<Menu.GroupLabel className={GROUP_LABEL}>
				{server ? (
					translate("Move to Local, {{server}}", { server })
				) : (
					<>
						{translate("Move to ")}
						{accountName(move.account)}
					</>
				)}
			</Menu.GroupLabel>
			{models.map((choice) => (
				<Menu.Item
					key={choice.id}
					className={MENU_ITEM}
					onClick={() => onPick(choice.id, move.account.id)}
				>
					<Icon aria-hidden className="size-3 text-muted-foreground" strokeWidth={2.4} />
					<span className="flex-1 text-[12.5px] font-[550]">{choice.label}</span>
					{kind === "local" && (
						<TagPill tag={local?.tag(move.account, choice.id) ?? null} />
					)}
				</Menu.Item>
			))}
		</Menu.Group>
	);
};

/**
 * The item that opens the Local models settings, after a separator when
 * `apart`, with items above it.
 */
const SetupItem = ({ onSetup, apart }: { onSetup: () => void; apart: boolean }) => {
	useI18n();
	return (
		<>
			{apart && <Menu.Separator className="mx-1 my-[5px] h-px bg-border" />}
			<Menu.Item className={MENU_ITEM} onClick={onSetup}>
				<Settings2 aria-hidden className="size-3 text-muted-foreground" strokeWidth={2.4} />
				<span className="flex-1 text-[12.5px]">{translate("Set up local models…")}</span>
			</Menu.Item>
		</>
	);
};

/**
 * Picks the agent's model from `choices`, what its account can run (see
 * `modelsOf`), or a model of one of `moves`, which moves the agent to that
 * account with its conversation.
 * Without a list to pick from, or with `onPick` left out, it only names
 * the model. `folded`, in a narrow footer, cuts its name short. `local`
 * lists a local agent's models under their server with a fit tag, and adds
 * a last item that opens the Local models settings. Past a dozen models in
 * all, as `showsModelSearch` says, a search field narrows every group to
 * the models whose name or ID has each word typed, as `filterModels` says,
 * and leaves out a group with none.
 */
export const ModelPicker = ({
	kind,
	model,
	account,
	choices,
	moves = [],
	place = "footer",
	folded = false,
	local,
	onPick,
}: {
	kind: AgentKind;
	model: string | null;
	account: Account | undefined;
	choices: readonly ModelChoice[];
	moves?: readonly Move[];
	place?: PickerPlace;
	folded?: boolean;
	local?: LocalMenu;
	onPick?: (model: string, account?: AccountId) => void;
}) => {
	useI18n();
	const [query, setQuery] = useState("");
	const Icon = MODEL_ICON[kind];
	const ownLocal = kind === "local" && account !== undefined ? local : undefined;
	const setup =
		local && (ownLocal || moves.some((m) => accountKind(m.account) === "local"))
			? local.onSetup
			: null;
	const label = modelLabel(model, choices);
	// A name cut short keeps four letters' width, so a shorter one stays whole.
	const cut = folded && label.length > 4;
	// The header names the vendor beside it already, so it leaves the icon out.
	const shown =
		place === "header" ? (
			label
		) : (
			<>
				<Icon aria-hidden className="size-3 shrink-0" strokeWidth={2.4} />
				<span className={cn("truncate", cut && "min-w-[4ch]")}>{label}</span>
			</>
		);
	// The footer's tooltip names the model the agent runs, when its name
	// says less, and its full name once that gets cut short.
	const id = model !== null && model !== label ? model : null;
	const tip =
		place === "footer" && (folded || id !== null) ? (
			<>
				<span className="block">{label}</span>
				{id !== null && <span className="block text-muted-foreground">{id}</span>}
			</>
		) : null;
	if (!account || (choices.length === 0 && moves.length === 0) || !onPick) {
		return (
			<Tooltip content={tip}>
				<span
					className={
						place === "header"
							? "inline-flex h-[22px] shrink-0 items-center px-1.5 text-[11.5px] whitespace-nowrap text-secondary-foreground"
							: cn(
									"inline-flex h-[26px] shrink-0 items-center gap-[5px] px-2 text-xs whitespace-nowrap text-secondary-foreground",
									cut && CUT_PLAIN
								)
					}
				>
					{shown}
				</span>
			</Tooltip>
		);
	}
	const current = currentModel(model, choices);
	const groups = moves.map((move) => ({ move, models: moveModels(move) }));
	const searching = showsModelSearch(
		groups.reduce((count, group) => count + group.models.length, choices.length)
	);
	const ownShown = searching ? filterModels(choices, query) : choices;
	const movesShown = searching
		? groups
				.map((group) => ({ ...group, models: filterModels(group.models, query) }))
				.filter((group) => group.models.length > 0)
		: groups;
	const none = ownShown.length === 0 && movesShown.length === 0;
	return (
		<Picker
			label={translate("Model: {{label}}", { label: label })}
			trigger={shown}
			place={place}
			fold={cut ? "shrink" : null}
			tip={tip}
			search={
				searching
					? {
							query,
							onQuery: setQuery,
							label: translate("Search models"),
							empty: none ? (
								<p
									role="status"
									className="px-2 py-3 text-[12px] text-muted-foreground"
								>
									{translate("No models match")}
								</p>
							) : null,
						}
					: null
			}
		>
			{ownShown.length > 0 && (
				<Menu.RadioGroup
					value={current}
					onValueChange={(value: string) => {
						if (value !== current) onPick(value);
					}}
				>
					<Menu.GroupLabel className={GROUP_LABEL}>
						{ownLocal ? (
							ownLocal.group(account)
						) : (
							<>
								{translate("Model, ")}
								{accountName(account)}
							</>
						)}
					</Menu.GroupLabel>
					{ownShown.map((choice) => (
						<Menu.RadioItem key={choice.id} value={choice.id} className={MENU_ITEM}>
							<span className="flex-1 text-[12.5px] font-[550] [[data-checked]_&]:font-semibold">
								{choice.label}
							</span>
							{choice.default && (
								<span className="text-[11px] text-muted-foreground">
									{translate("Default")}
								</span>
							)}
							{ownLocal && <TagPill tag={ownLocal.tag(account, choice.id)} />}
							<Tick />
						</Menu.RadioItem>
					))}
				</Menu.RadioGroup>
			)}
			{movesShown.map(({ move, models }, at) => (
				<Fragment key={move.account.id}>
					{(ownShown.length > 0 || at > 0) && (
						<Menu.Separator className="mx-1 my-[5px] h-px bg-border" />
					)}
					<MoveGroup move={move} models={models} local={local} onPick={onPick} />
				</Fragment>
			))}
			{setup && <SetupItem onSetup={setup} apart={!none} />}
		</Picker>
	);
};

/**
 * Picks the agent's permission mode, showing the one it really runs in.
 * `acp` is what a local ACP agent can do, which hides the modes it can't.
 * `folded`, in a narrow footer, shows its icon alone.
 */
export const PermissionPicker = ({
	vendor,
	acp,
	permission,
	folded = false,
	onPick,
}: {
	vendor: Vendor;
	acp?: AcpFeatures | null;
	permission: AgentPermission;
	folded?: boolean;
	onPick: (mode: PermissionMode) => void;
}) => {
	useI18n();
	const running = runningMode(permission);
	const note = permissionNote(permission);
	const label = translate("Permission mode: {{label}}", { label: permissionLabel(running) });
	return (
		<Picker
			label={label}
			fold={folded ? "icon" : null}
			tip={folded ? label : null}
			trigger={
				<>
					<Shield aria-hidden className="size-[13px]" strokeWidth={2} />
					{!folded && permissionLabel(running)}
				</>
			}
		>
			<Menu.RadioGroup
				value={running}
				onValueChange={(value: PermissionMode) => {
					if (value !== running) onPick(value);
				}}
			>
				{permissionChoices(vendor, acp).map(({ mode, label, detail, offered }) => (
					<Menu.RadioItem
						key={mode}
						value={mode}
						disabled={!offered}
						className={MENU_ITEM}
					>
						<span className="flex flex-1 flex-col gap-px">
							<span className="text-[12.5px] font-[550] [[data-checked]_&]:font-semibold">
								{label}
							</span>
							<span
								className={cn(
									"text-[11px]",
									mode === "full_access"
										? "text-status-busy-text"
										: "text-muted-foreground"
								)}
							>
								{offered ? detail : NOT_OFFERED}
							</span>
						</span>
						<Tick />
					</Menu.RadioItem>
				))}
			</Menu.RadioGroup>
			{note && (
				<p className="mt-[5px] border-t border-track px-2 pt-[7px] pb-1 text-[11px] text-muted-foreground">
					{note}
				</p>
			)}
		</Picker>
	);
};
