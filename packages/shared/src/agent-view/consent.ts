// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// How a card that asks for your consent lays out what it covers, and turns
// what you pick into an answer. maxd puts the same in the question's text
// for a client that shows only that.

import type { AttentionAnswer, ConsentRow, Question } from "../bindings";

/** What a value that looks secret shows until you ask to see it. */
export const HIDDEN_VALUE = "••••••••";

/** The rows under one heading, such as the workflow that names them. */
export type ConsentGroup = {
	readonly name: string;
	readonly rows: readonly ConsentRow[];
};

/** The rows under each heading, in the order the headings first come. */
export const consentGroups = (rows: readonly ConsentRow[]): ConsentGroup[] => {
	const groups = new Map<string, ConsentRow[]>();
	for (const row of rows) {
		const group = groups.get(row.group);
		if (group) group.push(row);
		else groups.set(row.group, [row]);
	}
	return [...groups].map(([name, grouped]) => ({ name, rows: grouped }));
};

/** What a row's value shows: a secret stays hidden until you ask to see it. */
export const shownValue = (row: ConsentRow, shown: boolean): string =>
	row.secret && !shown ? HIDDEN_VALUE : row.value;

/** The answer for the option you picked, with the Remember box when you ticked it. */
export const consentAnswer = (
	question: Question,
	label: string,
	remember: boolean
): AttentionAnswer => {
	const answers = [{ id: question.id, values: [label] }];
	const box = question.consent?.remember;
	if (remember && box) answers.push({ id: box.id, values: [box.label] });
	return { type: "questions", answers };
};
