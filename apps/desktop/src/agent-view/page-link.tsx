// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Links to claude.ai pages. One opens in the browser only when you click
// it, and only when it's an https link on claude.ai, checked again on the
// click. Anything else shows as plain text.

import { Fragment, type MouseEvent, type ReactNode } from "react";
import { SquareArrowOutUpRight } from "lucide-react";

import { claudeLink } from "@maxhq/shared/agent-view/artifact-line";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

import { CopyButton } from "@/components/copy-button";
import { ROW_BUTTON } from "./open-in";

const LINK = "text-primary underline-offset-2 hover:underline";

const COPY =
	"inline-flex h-[22px] min-w-[22px] shrink-0 cursor-pointer items-center justify-center gap-1 rounded-md px-[5px] text-[11.5px] text-muted-foreground hover:bg-accent hover:text-foreground";

// Lets the browser open the link only when it still checks out.
const checked = (event: MouseEvent<HTMLAnchorElement>) => {
	if (claudeLink(event.currentTarget.getAttribute("href")) === null) event.preventDefault();
};

/** `url` as a link when it's a claude.ai page, and as plain text when not. */
export const PageLink = ({ url, children }: { url: string; children?: ReactNode }) => {
	const href = claudeLink(url);
	if (href === null) return <>{children ?? url}</>;
	return (
		<a href={href} target="_blank" rel="noreferrer" onClick={checked} className={LINK}>
			{children ?? url}
		</a>
	);
};

/** Text with each line that is only a claude.ai link made a link. */
export const LinkedLines = ({ text }: { text: string }) =>
	text.split("\n").map((line, index) => (
		<Fragment key={index}>
			{index > 0 ? "\n" : null}
			{claudeLink(line.trim()) ? <PageLink url={line.trim()}>{line}</PageLink> : line}
		</Fragment>
	));

/**
 * A row's buttons for a claude.ai page: one that opens it in the browser,
 * with the link as its tooltip, and one that copies the link. Nothing for
 * a link that isn't on claude.ai.
 */
export const PageButtons = ({ url, name }: { url: string; name: string }) => {
	useI18n();
	const href = claudeLink(url);
	if (href === null) return null;
	return (
		<>
			<a
				href={href}
				target="_blank"
				rel="noreferrer"
				onClick={checked}
				title={href}
				aria-label={translate("Open {{name}} in the browser", { name })}
				className={ROW_BUTTON}
			>
				<SquareArrowOutUpRight size={12} strokeWidth={1.9} aria-hidden />
			</a>
			<CopyButton text={href} label="" title={translate("Copy link")} className={COPY} />
		</>
	);
};
