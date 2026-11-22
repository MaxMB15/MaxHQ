// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The menu on a command's mark in a terminal whose shell marks its
// commands: Send to agent sends what the command printed as an Output pill,
// and the copy items copy that or the command you typed.

import { Menu } from "@base-ui/react/menu";

import { useServices } from "@/app/services";
import { MENU_ITEM, MENU_POPUP } from "@/composer/pickers";
import { cn } from "@/lib/utils";
import { t, useI18n } from "@maxhq/shared/i18n";
import type { TerminalEntity } from "@maxhq/shared/terminal/select";

import { addPicked } from "./actions";
import { registryOf } from "./live";

/** A command whose mark you clicked, and the mark the menu sits by. */
export type MarkedAt = { readonly id: number; readonly anchor: Element };

const copyText = (text: string) => void navigator.clipboard?.writeText(text).catch(() => {});

/** The menu for the command at `marked`, while the view still holds it. */
export const CommandMenu = ({
	terminal,
	marked,
	own,
	close,
}: {
	terminal: TerminalEntity;
	marked: MarkedAt;
	/** Whether the terminal's agent takes Send to agent, which one you only watch doesn't. */
	own: boolean;
	close: () => void;
}) => {
	useI18n();
	const services = useServices();
	const view = registryOf(services)?.view(terminal.id) ?? null;
	const output = view?.output(marked.id) ?? null;
	if (!view || !output) return null;
	const { mark } = output;
	const status = mark.running
		? t("Running")
		: mark.exit === null
			? null
			: t("Exit {{exit_code}}", { exit_code: mark.exit });
	return (
		<Menu.Root
			open
			onOpenChange={(open) => {
				if (!open) close();
			}}
		>
			<Menu.Portal>
				<Menu.Positioner
					anchor={marked.anchor}
					side="bottom"
					align="start"
					sideOffset={4}
					// Output that scrolls the mark away takes the menu with it.
					className="z-[130] data-[anchor-hidden]:hidden"
				>
					<Menu.Popup
						className={cn(
							MENU_POPUP,
							"w-auto max-w-[320px] min-w-[200px] font-sans text-[12.5px]"
						)}
					>
						{mark.command || status ? (
							<div className="flex items-center gap-3 px-2 pt-[5px] pb-1 text-[11px]">
								<span className="min-w-0 flex-1 truncate font-mono text-muted-foreground">
									{mark.command}
								</span>
								{status ? (
									<span
										className={cn(
											"shrink-0",
											mark.exit
												? "text-status-error-text"
												: "text-subtle-foreground"
										)}
									>
										{status}
									</span>
								) : null}
							</div>
						) : null}
						{own ? (
							<Menu.Item
								className={MENU_ITEM}
								onClick={() =>
									addPicked(
										services,
										terminal.agent,
										terminal,
										output,
										view.text(0).total,
										true
									)
								}
							>
								{t("Send to agent")}
							</Menu.Item>
						) : null}
						<Menu.Item
							className={MENU_ITEM}
							disabled={output.text === ""}
							onClick={() => copyText(output.text)}
						>
							{t("Copy output")}
						</Menu.Item>
						<Menu.Item
							className={MENU_ITEM}
							disabled={mark.command === ""}
							onClick={() => copyText(mark.command)}
						>
							{t("Copy command")}
						</Menu.Item>
					</Menu.Popup>
				</Menu.Positioner>
			</Menu.Portal>
		</Menu.Root>
	);
};
