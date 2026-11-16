// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A fenced code block in a message: a header with the language, a copy
// button and, for commands in a reply, Run, then the code, plain at first
// and highlighted once Shiki has it.

import { useEffect, useState } from "react";

import { CopyButton } from "@/components/copy-button";
import { cn } from "@/lib/utils";

import { cachedLines, highlight, languageOf, type Lines } from "./highlight";
import { RunMenu } from "./run-menu";
import { t as translate, useI18n } from "@maxhq/shared/i18n";

// Highlighted lines for code, once they're ready. Code still streaming in
// stays plain, so it isn't highlighted again on every chunk.
const useLines = (code: string, lang: string | null, live: boolean): Lines | null => {
	const language = languageOf(lang);
	const cached = language ? (cachedLines(code, language) ?? null) : null;
	const [lines, setLines] = useState<{ code: string; lines: Lines } | null>(null);

	useEffect(() => {
		if (!language || live || cached) return;
		let current = true;
		void highlight(code, language).then((result) => {
			if (current && result) setLines({ code, lines: result });
		});
		return () => {
			current = false;
		};
	}, [code, language, live, cached]);

	if (cached) return cached;
	return lines?.code === code ? lines.lines : null;
};

/** A code block with its header. */
export const CodeBlock = ({
	code,
	lang,
	title,
	live = false,
	className,
}: {
	code: string;
	/** The fence's language, if it named one. */
	lang: string | null;
	/** What the header says, the language unless given. */
	title?: string;
	/** Set while the block is still streaming in. */
	live?: boolean;
	className?: string;
}) => {
	useI18n();
	const text = code.replace(/\n$/, "");
	const lines = useLines(text, lang, live);
	return (
		<div
			className={cn("overflow-hidden rounded-lg border border-border bg-subpanel", className)}
		>
			<div
				data-find-skip
				className="flex h-[30px] items-center gap-2 border-b border-border pr-1.5 pl-3 font-mono text-[11.5px] text-muted-foreground select-none"
			>
				<span className="truncate">{title ?? lang ?? translate("Code")}</span>
				<CopyButton text={text} />
				{live ? null : <RunMenu code={text} lang={lang} />}
			</div>
			<pre className="overflow-x-auto px-3 py-2.5 font-mono text-[11.5px] leading-[1.6] text-terminal select-text">
				<code>
					{lines
						? lines.map((line, i) => (
								// Lines have no identity but their place, and the list is rebuilt whole.
								<span key={i} data-line className="block min-h-[1.6em]">
									{line.map((token, j) => (
										<span
											key={j}
											style={{
												color: token.color,
												fontStyle:
													token.fontStyle && token.fontStyle & 1
														? "italic"
														: undefined,
												fontWeight:
													token.fontStyle && token.fontStyle & 2
														? 600
														: undefined,
											}}
										>
											{token.content}
										</span>
									))}
								</span>
							))
						: text}
				</code>
			</pre>
		</div>
	);
};
