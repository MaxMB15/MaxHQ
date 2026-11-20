// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A fenced code block in a message, as on the desktop: a header with the
// language and Copy, then the code, plain at first and coloured once Shiki
// has it. Code still streaming in stays plain, so it isn't coloured again
// on every chunk.

import { useI18n } from "@maxhq/shared/i18n";

import { Fragment, memo, useMemo } from "react";
import { Text } from "react-native";

import { CodeCard, CodeText, CopyButton } from "./CodeCard";
import { languageOf, type Lines, tokenStyle, useHighlight } from "./highlight";

const Coloured = ({ lines }: { lines: Lines }) =>
	lines.map((line, i) => (
		// Lines have no identity but their place, and the list is rebuilt whole.
		<Fragment key={i}>
			{line.map((token, j) => (
				<Text key={j} style={tokenStyle(token)}>
					{token.content}
				</Text>
			))}
			{i < lines.length - 1 ? "\n" : null}
		</Fragment>
	));

/** A code block with its header. `live` is set while it's still streaming in. */
export const CodeBlock = memo(
	({ code, lang, live = false }: { code: string; lang: string | null; live?: boolean }) => {
		const { t } = useI18n();
		const language = useMemo(() => languageOf(lang), [lang]);
		const lines = useHighlight(code, language, !live);
		return (
			<CodeCard name={lang ?? t("Code")} buttons={<CopyButton text={code} />}>
				<CodeText>{lines ? <Coloured lines={lines} /> : code}</CodeText>
			</CodeCard>
		);
	}
);
CodeBlock.displayName = "CodeBlock";
