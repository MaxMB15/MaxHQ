// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What went wrong in a visual's page, gathered from its frame's reports for
// the bars over it: its errors, the hosts the network rule blocked and the
// files it asked for that aren't there. Load this time lets some of those
// hosts through, and only those it can help.

import { type Blocked, blockedLoad, MOST_HOSTS } from "./frame";

/** An error a visual's page threw, with the line it came from when known. */
export type VisualError = { readonly message: string; readonly line: number | null };

// The errors kept to show, and how many there were.
const KEPT_ERRORS = 20;

/** What a visual's page reported going wrong, so far. */
export type VisualReports = {
	/** The page's errors, each message once and only the first few. */
	readonly errors: readonly VisualError[];
	/** How many errors came in all. */
	readonly errorCount: number;
	/** The blocked hosts, by host. */
	readonly blocked: ReadonlyMap<string, Blocked>;
	/** The files the page named that aren't there, as it wrote them. */
	readonly missing: readonly string[];
};

/** Nothing reported yet. */
export const NO_REPORTS: VisualReports = {
	errors: [],
	errorCount: 0,
	blocked: new Map(),
	missing: [],
};

/** One thing a page reported going wrong. */
export type VisualReport =
	| { readonly type: "error"; readonly error: VisualError }
	| { readonly type: "blocked"; readonly url: string; readonly what: string }
	| { readonly type: "missing"; readonly path: string };

/** The reports with one more, for a reducer. */
export const addReport = (reports: VisualReports, report: VisualReport): VisualReports => {
	switch (report.type) {
		case "error": {
			const known = reports.errors.some((e) => e.message === report.error.message);
			return {
				...reports,
				errorCount: reports.errorCount + 1,
				errors:
					known || reports.errors.length >= KEPT_ERRORS
						? reports.errors
						: [...reports.errors, report.error],
			};
		}
		case "blocked": {
			const blocked = blockedLoad(report.url, report.what);
			if (!blocked) return reports;
			const before = reports.blocked.get(blocked.host);
			// A host is loadable when anything it was asked for would be.
			if (before && (before.loadable || !blocked.loadable)) return reports;
			return { ...reports, blocked: new Map(reports.blocked).set(blocked.host, blocked) };
		}
		case "missing":
			return reports.missing.includes(report.path)
				? reports
				: { ...reports, missing: [...reports.missing, report.path] };
	}
};

/**
 * The blocked hosts Load this time would let through: the loadable ones not
 * through yet, as many as there's room for beside `hosts`, already through.
 */
export const loadableHosts = (
	blocked: ReadonlyMap<string, Blocked>,
	hosts: readonly string[]
): string[] =>
	[...blocked.values()]
		.filter((b) => b.loadable && !hosts.includes(b.host))
		.map((b) => b.host)
		.slice(0, Math.max(0, MOST_HOSTS - hosts.length));

/** `hosts` with `more` let through too, up to the most a visual takes. */
export const withHosts = (hosts: readonly string[], more: readonly string[]): readonly string[] =>
	[...hosts, ...more.filter((host) => !hosts.includes(host))].slice(0, MOST_HOSTS);
