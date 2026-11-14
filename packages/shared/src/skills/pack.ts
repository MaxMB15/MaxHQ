// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// A project's devflow pack, as its Skills tab shows it: which library
// sources can be one, a lock with a pack picked, moved or taken out, and
// where each checkout's copy stands.

import { t } from "../i18n";

import type {
	CheckoutSkills,
	SkillLibrary,
	SkillLock,
	SkillSource,
	SkillSourceId,
} from "../bindings";

/** The library sources a project can use as its pack, by name. */
export const packSources = (library: SkillLibrary): SkillSource[] =>
	library.sources
		.filter((source) => source.pack && source.commit)
		.sort((a, b) => a.name.localeCompare(b.name));

/** `lock` with the source `id` as its pack at the source's newest tree, or with none. */
export const withPack = (
	library: SkillLibrary,
	lock: SkillLock,
	id: SkillSourceId | null
): SkillLock => {
	const source = id ? packSources(library).find((s) => s.id === id) : undefined;
	const next = { ...lock };
	delete next.pack;
	if (source) next.pack = { source: source.id, commit: source.commit!, tree: source.pack! };
	return next;
};

/** Whether the lock's pack has a newer tree in its source. */
export const packHasUpdate = (library: SkillLibrary, lock: SkillLock): boolean => {
	const source = lock.pack && packSources(library).find((s) => s.id === lock.pack!.source);
	return !!source && source.pack !== lock.pack!.tree;
};

/** A tree as the tab shows it, by its first seven characters. */
export const shortTree = (tree: string): string => tree.slice(0, 7);

/** Where one checkout's pack stands, in words, or null when it has none. */
export const packStateText = (checkout: CheckoutSkills): string | null => {
	const pack = checkout.pack;
	if (!pack) return null;
	switch (pack.state) {
		case "installing":
			return t("Installing {{shortTree_pack_tree}}", {
				shortTree_pack_tree: shortTree(pack.tree),
			});
		case "installed":
			return t("Installed at {{shortTree_pack_tree}}", {
				shortTree_pack_tree: shortTree(pack.tree),
			});
		case "failed":
			return t("Install of {{shortTree_pack_tree}} failed", {
				shortTree_pack_tree: shortTree(pack.tree),
			});
		case "vendored":
			return t("Vendored by someone else, so MaxHQ leaves it alone");
	}
};
