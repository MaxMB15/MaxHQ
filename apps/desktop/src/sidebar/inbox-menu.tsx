// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The inbox menus: the one a right-click, Shift+F10 or the ⋯ on an agent's
// row opens, and the Actions menu for the agents you selected. Both offer
// the same things, for one agent or several: settle, snooze, pin, whether
// auto-settle may settle them, and delete for the ones MaxHQ holds. A
// right-click on a selected row acts on the whole selection, and on one
// agent the row's menu also renames it, opens its folder in an editor and
// copies its folder path, branch or session ID.

import { useI18n, formatNumber } from "@maxhq/shared/i18n";

import { ContextMenu } from "@base-ui/react/context-menu";
import { Menu } from "@base-ui/react/menu";
import {
	AlarmClock,
	AlarmClockOff,
	Check,
	ChevronDown,
	ChevronRight,
	CircleCheck,
	Copy,
	Ellipsis,
	Pencil,
	Pin,
	PinOff,
	Trash2,
	Undo2,
	X,
} from "lucide-react";
import { type ReactElement, type ReactNode, useRef, useState } from "react";

import { CopyItem } from "@/agent-view/header-menu";
import { type EditorChoice, OpenInItem, OpenInSetup } from "@/agent-view/open-in";
import { confirmAndDelete } from "@/app/delete-agents";
import { useServices, useUi, useView } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer";
import { keyHint, SHORTCUTS } from "@/layout/shortcuts";
import { cn } from "@/lib/utils";
import type { AgentId, Timestamp } from "@maxhq/shared/bindings";
import { formatClock, formatWhen } from "@maxhq/shared/status/format";
import { deletableAgents } from "@maxhq/shared/ui/delete-agents";
import { showFailure } from "@maxhq/shared/ui/inbox-notice";
import {
	inboxMenu,
	inboxTargets,
	limitSnoozedText,
	type InboxStep,
} from "@maxhq/shared/view/inbox-actions";
import { sessionIdLabel } from "@maxhq/shared/view/copy-agent";
import { pickSnoozePreset, resolveSnoozePresets } from "@maxhq/shared/view/snooze";

import { runChoice, runSteps } from "./commands";
import { requestCustomSnooze } from "./custom-snooze";
import { noticesOf } from "./notices";
import { usePrefs } from "./prefs";

const ICON = "shrink-0 text-secondary-foreground";
const LABEL = "min-w-0 flex-1 truncate text-[12.5px] font-[550]";
const HINT = "shrink-0 text-[11px] text-muted-foreground";
const SEPARATOR = "mx-1 my-[5px] h-px bg-border";
const POPUP = cn(MENU_POPUP, "w-[250px]");

const formatTime = (date: Date): string => formatClock(date.getTime());

/** What the menu says when a snooze time passed while it was open. */
export const PASSED = "That snooze time has passed. Choose another time.";

const Item = ({
	icon,
	label,
	hint,
	danger,
	onClick,
}: {
	icon?: ReactNode;
	label: string;
	hint?: string;
	/** Whether it throws something away. */
	danger?: boolean;
	onClick: () => void;
}) => {
	useI18n();
	return (
		<Menu.Item onClick={onClick} className={MENU_ITEM}>
			{icon ?? <span aria-hidden className="size-[13px] shrink-0" />}
			<span className={cn(LABEL, danger && "text-status-error-text")}>{label}</span>
			{hint ? <span className={HINT}>{hint}</span> : null}
		</Menu.Item>
	);
};

const Submenu = ({
	icon,
	label,
	children,
}: {
	icon: ReactNode;
	label: string;
	children: ReactNode;
}) => {
	useI18n();
	return (
		<Menu.SubmenuRoot>
			<Menu.SubmenuTrigger className={MENU_ITEM}>
				{icon}
				<span className={LABEL}>{label}</span>
				<ChevronRight aria-hidden size={12} strokeWidth={2.4} className={ICON} />
			</Menu.SubmenuTrigger>
			<Menu.Portal>
				<Menu.Positioner side="right" align="start" sideOffset={4} className="z-[130]">
					<Menu.Popup className={POPUP}>{children}</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.SubmenuRoot>
	);
};

/**
 * The menu's items for these agents. `now` is the render's clock, which the
 * snooze times are worked out from. `onRename` puts Rename first, for one
 * agent, sub-agents included, and `children` go before Delete.
 */
export const InboxMenuItems = ({
	ids,
	now,
	onRename,
	children,
}: {
	ids: readonly AgentId[];
	now: Timestamp;
	onRename?: () => void;
	children?: ReactNode;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const view = useView((s) => s.view);
	const presets = usePrefs((p) => p.snoozePresets);
	const targets = inboxTargets(view, ids);
	const rename = onRename ? (
		<Item
			icon={<Pencil aria-hidden size={13} strokeWidth={2} className={ICON} />}
			label={t("Rename")}
			onClick={onRename}
		/>
	) : null;
	if (targets.length === 0)
		return (
			<>
				{rename}
				{rename && children ? <Menu.Separator className={SEPARATOR} /> : null}
				{children}
			</>
		);
	const menu = inboxMenu(view, targets, now);
	const several = targets.length > 1;
	const shown = resolveSnoozePresets(new Date(now), { presets, formatTime });

	const snooze = (agents: readonly AgentId[], until: Timestamp) =>
		runSteps(services, [{ agents, change: { change: "snooze", until } }]);
	const pickPreset = (agents: readonly AgentId[], id: string) => {
		const preset = pickSnoozePreset({ shown, id, now: new Date(now), presets, formatTime });
		if (preset) snooze(agents, preset.until);
		else showFailure(noticesOf(services), t(PASSED));
	};
	const custom = (agents: readonly AgentId[]) => {
		void requestCustomSnooze(services).then((until) => {
			if (until !== null) snooze(agents, until);
		});
	};
	const untilLimit = (steps: readonly InboxStep[]) =>
		runSteps(services, steps, steps.length > 1 ? limitSnoozedText : undefined);

	const deletable = deletableAgents(view, ids);
	const remove = () => {
		void confirmAndDelete(services, deletable).then((failures) => {
			if (failures.length > 0) noticesOf(services).store.setState({ failures });
		});
	};

	const settling = menu.settle?.steps[0]?.change.change === "settle";
	const pinning = menu.pin?.steps[0]?.change.change === "pin";
	const limitSteps = menu.untilLimit?.steps ?? [];
	const limitAt = limitSteps.length === 1 ? limitSteps[0]!.change : null;

	return (
		<>
			{rename ? (
				<>
					{rename}
					<Menu.Separator className={SEPARATOR} />
				</>
			) : null}
			{menu.settle ? (
				<Item
					icon={
						settling ? (
							<CircleCheck aria-hidden size={13} strokeWidth={2} className={ICON} />
						) : (
							<Undo2 aria-hidden size={13} strokeWidth={2} className={ICON} />
						)
					}
					label={menu.settle.label}
					hint={settling ? keyHint(SHORTCUTS.settle) : undefined}
					onClick={() => runChoice(services, menu.settle)}
				/>
			) : null}
			{menu.wake ? (
				<Item
					icon={<AlarmClockOff aria-hidden size={13} strokeWidth={2} className={ICON} />}
					label={menu.wake.label}
					onClick={() => runChoice(services, menu.wake)}
				/>
			) : null}
			{menu.snooze ? (
				<Submenu
					icon={<AlarmClock aria-hidden size={13} strokeWidth={2} className={ICON} />}
					label={
						several
							? t("Snooze ({{length}})", { length: formatNumber(menu.snooze.length) })
							: t("Snooze")
					}
				>
					{shown.map((preset) => (
						<Item
							key={preset.id}
							label={`${preset.label} (${preset.whenLabel})`}
							onClick={() => pickPreset(menu.snooze!, preset.id)}
						/>
					))}
					{menu.untilLimit ? (
						<Item
							label={
								limitAt?.change === "snooze"
									? `${menu.untilLimit.label} (${formatWhen(limitAt.until, now)})`
									: menu.untilLimit.label
							}
							onClick={() => untilLimit(limitSteps)}
						/>
					) : null}
					{shown.length > 0 || menu.untilLimit ? (
						<Menu.Separator className={SEPARATOR} />
					) : null}
					<Item label={t("Custom…")} onClick={() => custom(menu.snooze!)} />
				</Submenu>
			) : null}
			{menu.pin ? (
				<Item
					icon={
						pinning ? (
							<Pin aria-hidden size={13} strokeWidth={2} className={ICON} />
						) : (
							<PinOff aria-hidden size={13} strokeWidth={2} className={ICON} />
						)
					}
					label={menu.pin.label}
					hint={keyHint(SHORTCUTS.pin)}
					onClick={() => runChoice(services, menu.pin)}
				/>
			) : null}
			<Menu.Separator className={SEPARATOR} />
			<Menu.CheckboxItem
				closeOnClick
				checked={menu.autoSettle === "on"}
				onCheckedChange={(on: boolean) =>
					runChoice(services, on ? menu.autoSettleOn : menu.autoSettleOff)
				}
				className={MENU_ITEM}
			>
				<span className="flex size-[13px] shrink-0 items-center justify-center">
					<Menu.CheckboxItemIndicator>
						<Check aria-hidden size={13} strokeWidth={2.6} className="text-primary" />
					</Menu.CheckboxItemIndicator>
				</span>
				<span className="flex min-w-0 flex-1 flex-col">
					<span className={LABEL}>{t("Auto-settle when idle")}</span>
					{menu.keptActive ? (
						<span className="text-[11px] text-muted-foreground">{menu.keptActive}</span>
					) : null}
				</span>
				{menu.autoSettle === "mixed" ? <span className={HINT}>{t("Some")}</span> : null}
			</Menu.CheckboxItem>
			{menu.continueOn || menu.continueOff ? (
				<Menu.CheckboxItem
					closeOnClick
					checked={menu.continueAtLimit === "on"}
					onCheckedChange={(on: boolean) =>
						runChoice(services, on ? menu.continueOn : menu.continueOff)
					}
					className={MENU_ITEM}
				>
					<span className="flex size-[13px] shrink-0 items-center justify-center">
						<Menu.CheckboxItemIndicator>
							<Check
								aria-hidden
								size={13}
								strokeWidth={2.6}
								className="text-primary"
							/>
						</Menu.CheckboxItemIndicator>
					</span>
					<span className={LABEL}>{t("Continue after the limit")}</span>
					{menu.continueAtLimit === "mixed" ? (
						<span className={HINT}>{t("Some")}</span>
					) : null}
				</Menu.CheckboxItem>
			) : null}
			{children ? (
				<>
					<Menu.Separator className={SEPARATOR} />
					{children}
				</>
			) : null}
			{deletable.length > 0 ? (
				<>
					<Menu.Separator className={SEPARATOR} />
					<Item
						icon={
							<Trash2
								aria-hidden
								size={13}
								strokeWidth={2}
								className="shrink-0 text-status-error-text"
							/>
						}
						label={
							several
								? t("Delete ({{length}})", {
										length: formatNumber(deletable.length),
									})
								: t("Delete")
						}
						danger
						onClick={remove}
					/>
				</>
			) : null}
		</>
	);
};

// Open in, and the Copy submenu, for one agent.
const AgentItems = ({
	agent,
	onAsk,
}: {
	agent: AgentId;
	onAsk: (editor: EditorChoice) => void;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const found = useView((s) => s.view.tables.agent.get(agent));
	const workspace = useView((s) =>
		found ? s.view.tables.workspace.get(found.workspace) : undefined
	);
	if (!found) return null;
	const path = workspace?.location.path;
	const session = found.vendor_session_id;
	return (
		<>
			{workspace ? (
				<OpenInItem
					workspace={workspace.id}
					onAsk={onAsk}
					onFail={(text) => showFailure(noticesOf(services), text)}
				/>
			) : null}
			{path || found.branch || session ? (
				<Submenu
					icon={<Copy aria-hidden size={13} strokeWidth={2} className={ICON} />}
					label={t("Copy")}
				>
					{path ? <CopyItem label={t("Folder path")} text={path} /> : null}
					{found.branch ? <CopyItem label={t("Branch")} text={found.branch} /> : null}
					{session ? (
						<CopyItem label={sessionIdLabel(found.vendor)} text={session} />
					) : null}
				</Submenu>
			) : null}
		</>
	);
};

// The row's own agent, or the whole selection when the row is in it.
const RowMenuItems = ({
	agent,
	now,
	onRename,
	onAsk,
}: {
	agent: AgentId;
	now: Timestamp;
	onRename: () => void;
	onAsk: (editor: EditorChoice) => void;
}) => {
	useI18n();

	const picked = useUi((s) => s.picked);
	const ids = picked.has(agent) ? [...picked] : [agent];
	const one = ids.length === 1;
	return (
		<InboxMenuItems ids={ids} now={now} onRename={one ? onRename : undefined}>
			{one ? <AgentItems agent={agent} onAsk={onAsk} /> : null}
		</InboxMenuItems>
	);
};

/** Whether a key opens a focused element's menu. */
export const opensMenu = (e: { key: string; shiftKey: boolean }): boolean =>
	e.key === "ContextMenu" || (e.shiftKey && e.key === "F10");

/**
 * Gives a row its menu. `children` is the row, which a right-click, Shift+F10
 * or a ⋯ button opens the menu from. The ⋯ shows on hover and on the
 * `selected` row, which leaves room for it. `onRename` turns the title into
 * a field.
 */
export const RowMenu = ({
	agent,
	now,
	selected,
	onRename,
	children,
}: {
	agent: AgentId;
	now: Timestamp;
	selected: boolean;
	onRename: () => void;
	children: ReactElement;
}) => {
	const { t } = useI18n();

	const services = useServices();
	const [open, setOpen] = useState(false);
	// An editor that asks about the SSH setup first. The menu has closed by
	// then, so the question lives here.
	const [asking, setAsking] = useState<EditorChoice | null>(null);
	const workspace = useView((s) => s.view.tables.agent.get(agent)?.workspace);
	const wrapper = useRef<HTMLDivElement>(null);
	const items = <RowMenuItems agent={agent} now={now} onRename={onRename} onAsk={setAsking} />;
	return (
		<div
			ref={wrapper}
			className="group/agent-row relative shrink-0"
			onKeyDown={(e) => {
				if (!opensMenu(e) || e.target !== wrapper.current?.firstElementChild) return;
				e.preventDefault();
				setOpen(true);
			}}
		>
			<ContextMenu.Root>
				<ContextMenu.Trigger render={children} />
				<ContextMenu.Portal>
					<ContextMenu.Positioner className="z-[130]">
						<ContextMenu.Popup className={POPUP}>{items}</ContextMenu.Popup>
					</ContextMenu.Positioner>
				</ContextMenu.Portal>
			</ContextMenu.Root>
			<Menu.Root open={open} onOpenChange={setOpen}>
				{/* For the pointer only: the keyboard has Shift+F10 on the row. */}
				<Menu.Trigger
					aria-hidden
					tabIndex={-1}
					title={t("More")}
					className={cn(
						"absolute top-1/2 right-1 grid size-5 -translate-y-1/2 place-items-center rounded-[5px] text-muted-foreground opacity-0 outline-none group-hover/agent-row:opacity-100 hover:bg-border hover:text-foreground data-popup-open:bg-border data-popup-open:text-foreground data-popup-open:opacity-100",
						// On hover it covers the row's end.
						selected ? "opacity-100" : "bg-accent"
					)}
				>
					<Ellipsis aria-hidden size={13} strokeWidth={2.2} />
				</Menu.Trigger>
				<Menu.Portal>
					<Menu.Positioner side="bottom" align="end" sideOffset={4} className="z-[130]">
						<Menu.Popup
							className={POPUP}
							// Back to the row, since the ⋯ hides once the pointer leaves.
							finalFocus={() => {
								const row = wrapper.current?.firstElementChild;
								return row instanceof HTMLElement ? row : false;
							}}
						>
							{items}
						</Menu.Popup>
					</Menu.Positioner>
				</Menu.Portal>
			</Menu.Root>
			{asking && workspace ? (
				<OpenInSetup
					workspace={workspace}
					editor={asking}
					onClose={() => setAsking(null)}
					onFail={(text) => showFailure(noticesOf(services), text)}
				/>
			) : null}
		</div>
	);
};

/** The bar for the agents you selected, with their Actions menu. Hidden when none are. */
export const SelectionBar = ({ now }: { now: Timestamp }) => {
	const { t } = useI18n();

	const picked = useUi((s) => s.picked);
	const clearPicked = useUi((s) => s.clearPicked);
	if (picked.size === 0) return null;
	return (
		<div className="mx-1 mt-1.5 flex h-[30px] shrink-0 items-center gap-1 rounded-[7px] border border-track pr-1 pl-2 text-xs">
			<span className="min-w-0 flex-1 truncate text-secondary-foreground">
				{t("{{countText}} agents selected", {
					count: picked.size,
					countText: formatNumber(picked.size),
				})}
			</span>
			<Menu.Root>
				<Menu.Trigger className="inline-flex h-[22px] shrink-0 items-center gap-1 rounded-md px-1.5 text-secondary-foreground outline-none hover:bg-border hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring data-popup-open:bg-border data-popup-open:text-foreground">
					{t("Actions")}
					<ChevronDown aria-hidden className="size-[9px]" strokeWidth={2.6} />
				</Menu.Trigger>
				<Menu.Portal>
					<Menu.Positioner side="top" align="end" sideOffset={6} className="z-[130]">
						<Menu.Popup className={POPUP}>
							<InboxMenuItems ids={[...picked]} now={now} />
						</Menu.Popup>
					</Menu.Positioner>
				</Menu.Portal>
			</Menu.Root>
			<button
				type="button"
				aria-label={t("Clear the selection")}
				title={t("Clear the selection (Esc)")}
				onClick={clearPicked}
				className="grid size-[22px] shrink-0 cursor-pointer place-items-center rounded-[5px] text-muted-foreground hover:bg-border hover:text-foreground"
			>
				<X aria-hidden size={12} strokeWidth={2.4} />
			</button>
		</div>
	);
};
