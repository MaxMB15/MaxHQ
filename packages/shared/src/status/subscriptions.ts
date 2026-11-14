// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The plans the usage popover, the phone's Usage card and the header meter
// show. One plan can be signed in on several devices: the Mac's own login, a
// setup-token or sign-in a container uses, the same login on a server. Only
// the logins that read usage have numbers, so the logins of one plan show as
// one, with the numbers of the login that read them last. A login that knows
// nothing yet stays out once the vendor has a plan that does.

import type { Account, LoginKind } from "../bindings";

/** One plan, with every login that uses it. */
export type Subscription = {
	/**
	 * What to show: the login with the newest reading, with the plan and
	 * email another of its logins knows when it doesn't.
	 */
	readonly account: Account;
	/** Every login on the plan, on every device, in the order given. */
	readonly logins: readonly Account[];
	/**
	 * Set when the plan's only logins are tokens or sign-ins, which never
	 * read usage themselves, and the login they came from isn't in sight.
	 */
	readonly borrowed: boolean;
};

/** Logins a device keeps for its containers, which read no usage. */
const BORROWED: ReadonlySet<LoginKind> = new Set([
	"setup_token",
	"chat_gpt_sign_in",
	"cursor_api_key",
]);

/** Whether a login reads no usage of its own. */
const isBorrowed = (account: Account): boolean => BORROWED.has(account.login ?? "config_dir");

/**
 * The plan a login belongs to: the one its token came from, or else the
 * vendor's login with the same email and organization. A login without an
 * email stands alone, and so does every ACP agent.
 */
const planKey = (account: Account, byId: ReadonlyMap<string, Account>): string => {
	if (account.vendor === "acp") return `id:${account.id}`;
	const who = (account.origin !== null && byId.get(account.origin)) || account;
	if (!who.email) return `id:${who.id}`;
	return `${who.vendor}:${who.email.toLowerCase()}:${who.organization ?? ""}`;
};

/** How much a login has to show, so the one with the most leads its plan. */
const rank = (account: Account): number =>
	account.usage ? 3 : account.usage_failure ? 2 : account.plan ? 1 : 0;

/**
 * Above zero when `a` should lead a plan over `b`: the one with the most to
 * show, then the newest reading, then a login of its own over a token.
 */
const compare = (a: Account, b: Account): number =>
	rank(a) - rank(b) ||
	(a.usage?.checked_at ?? 0) - (b.usage?.checked_at ?? 0) ||
	Number(isBorrowed(b)) - Number(isBorrowed(a));

/** Whether a plan's logins have nothing at all to show. */
const knowsNothing = (logins: readonly Account[]): boolean =>
	logins.every((a) => rank(a) === 0 && !a.email && !isBorrowed(a));

/**
 * The plans `accounts` sign in to, in the order of their first login. A plan
 * whose logins know nothing, such as a config folder nobody signed in to, is
 * left out once the same vendor has a plan that knows something.
 */
export const subscriptions = (accounts: readonly Account[]): Subscription[] => {
	const byId = new Map(accounts.map((a) => [a.id, a]));
	const groups = new Map<string, Account[]>();
	for (const account of accounts) {
		const key = planKey(account, byId);
		const group = groups.get(key);
		if (group) group.push(account);
		else groups.set(key, [account]);
	}
	const plans = [...groups.values()].map((logins): Subscription => {
		const lead = logins.reduce((best, a) => (compare(a, best) > 0 ? a : best));
		const account: Account = {
			...lead,
			plan: lead.plan ?? logins.find((a) => a.plan)?.plan ?? null,
			email: lead.email ?? logins.find((a) => a.email)?.email ?? null,
		};
		return { account, logins, borrowed: !lead.usage && logins.every(isBorrowed) };
	});
	const known = new Set(
		plans.filter((p) => !knowsNothing(p.logins)).map((p) => p.account.vendor)
	);
	return plans.filter((p) => !knowsNothing(p.logins) || !known.has(p.account.vendor));
};
