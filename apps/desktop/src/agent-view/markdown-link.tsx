// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A link in Markdown. A web link opens in the browser. A file link opens
// the file in MaxHQ's viewer, read on the agent's device and at its line.
// A link within the message moves to its target there. A link that can't
// go anywhere shows as plain text and says why on hover, rather than
// looking like a link that does nothing.

import {
	type ComponentProps,
	createContext,
	type ReactNode,
	use,
	useLayoutEffect,
	useRef,
	useState,
} from "react";

import { useServicesIfAny } from "@/app/services";
import { panesOf } from "@/layout/panes";
import { useRowIfAny } from "@maxhq/shared/agent-view/context";
import { headingSlug, linkTarget, pathFrom } from "@maxhq/shared/agent-view/link-target";
import { formatNumber, useI18n } from "@maxhq/shared/i18n";

const LINK = "text-primary hover:underline";

/** What a link keeps of the attributes Markdown gives it, such as a footnote's id. */
type Attributes = Omit<ComponentProps<"a">, "href" | "children" | "target" | "onClick">;

/**
 * The folder a relative file link is read from, such as a Markdown file's
 * own. In a reply it's empty, and maxd reads the path from the agent's
 * folder.
 */
export const LinkFolder = createContext("");

/** The element that holds one message's Markdown. */
const MESSAGE = '[data-find="markdown"]';

/** The element a link within the message names, if the message has it. */
const anchoredIn = (message: Element | null, id: string): Element | null => {
	if (!message) return null;
	for (const element of message.querySelectorAll("[id]")) {
		// Footnotes and ids in raw HTML carry the message's prefix.
		if (element.id === id || (element.id.startsWith("md-") && element.id.endsWith(`-${id}`)))
			return element;
	}
	const slug = headingSlug(id);
	for (const heading of message.querySelectorAll("h1, h2, h3, h4, h5, h6")) {
		if (headingSlug(heading.textContent ?? "") === slug) return heading;
	}
	return null;
};

const Plain = ({ href, children }: { href: string | undefined; children: ReactNode }) => {
	const { t } = useI18n();
	return (
		<span
			className="cursor-help underline decoration-muted-foreground/40 decoration-dotted"
			title={t("MaxHQ can't open this link: {{target}}", { target: href ?? "" })}
		>
			{children}
		</span>
	);
};

const Anchor = ({
	attributes,
	id,
	href,
	children,
}: {
	attributes: Attributes;
	id: string;
	href: string | undefined;
	children: ReactNode;
}) => {
	const place = useRef<HTMLSpanElement>(null);
	// It draws as a link until it has looked, so a footnote never flickers.
	const [found, setFound] = useState(true);
	// Looks again as the message changes, since one streaming in can add
	// the target later.
	useLayoutEffect(() => {
		const message = place.current?.closest(MESSAGE) ?? null;
		const look = () => setFound(anchoredIn(message, id) !== null);
		look();
		if (!message) return;
		const watch = new MutationObserver(look);
		watch.observe(message, { childList: true, subtree: true });
		return () => watch.disconnect();
	}, [id]);
	return (
		<span ref={place} className="contents">
			{found ? (
				<Link attributes={attributes} id={id}>
					{children}
				</Link>
			) : (
				<Plain href={href}>{children}</Plain>
			)}
		</span>
	);
};

const Link = ({
	attributes,
	id,
	children,
}: {
	attributes: Attributes;
	id: string;
	children: ReactNode;
}) => (
	<a
		{...attributes}
		href={`#${id}`}
		className={LINK}
		onClick={(event) => {
			event.preventDefault();
			anchoredIn(event.currentTarget.closest(MESSAGE), id)?.scrollIntoView({
				block: "nearest",
			});
		}}
	>
		{children}
	</a>
);

/** A Markdown link, opened the way its target needs. */
export const MarkdownLink = ({
	href,
	children,
	...attributes
}: Attributes & { href?: string; children?: ReactNode }) => {
	const { t } = useI18n();
	const row = useRowIfAny();
	const services = useServicesIfAny();
	const folder = use(LinkFolder);
	const target = linkTarget(href);
	switch (target.kind) {
		case "web":
			return (
				<a
					{...attributes}
					href={target.url}
					target="_blank"
					rel="noreferrer"
					className={LINK}
				>
					{children}
				</a>
			);
		case "anchor":
			return (
				<Anchor attributes={attributes} id={target.id} href={href}>
					{children}
				</Anchor>
			);
		case "file": {
			if (!row || !services) return <Plain href={href}>{children}</Plain>;
			const path = pathFrom(folder, target.path);
			const { line } = target;
			return (
				<a
					{...attributes}
					href={`#${encodeURIComponent(path)}`}
					className={LINK}
					title={
						line === null
							? t("Open {{name}}", { name: path })
							: t("Open {{name}} at line {{line}}", {
									name: path,
									line: formatNumber(line, { useGrouping: false }),
								})
					}
					onClick={(event) => {
						event.preventDefault();
						panesOf(services).openFile(row.agent, path, line === null ? {} : { line });
					}}
				>
					{children}
				</a>
			);
		}
		case "none":
			return <Plain href={href}>{children}</Plain>;
	}
};
