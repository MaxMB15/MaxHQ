// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { discarder, ownCopy } from "./picked-copies";

const CACHE = "/var/mobile/Containers/Data/Application/APP/Library/Caches";

describe("ownCopy", () => {
	it("takes a copy in either picker's folder", () => {
		expect(ownCopy(`${CACHE}/ImagePicker/1F2E.jpg`, CACHE)).toBe(true);
		expect(ownCopy(`${CACHE}/DocumentPicker/3A4B.pdf`, CACHE)).toBe(true);
		expect(ownCopy(`${CACHE}/DocumentPicker/3A4B/report final.pdf`, CACHE)).toBe(true);
	});

	it("takes a caches folder given with a slash at its end", () => {
		expect(ownCopy(`${CACHE}/ImagePicker/1F2E.jpg`, `${CACHE}/`)).toBe(true);
	});

	it("never takes a file outside the pickers' folders", () => {
		expect(ownCopy(`${CACHE}/maxhq/downloads/notes.md`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/shared/photo.jpg`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/photo.jpg`, CACHE)).toBe(false);
		expect(ownCopy("/var/mobile/Media/DCIM/100APPLE/IMG_0001.HEIC", CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/ImagePickerOld/1F2E.jpg`, CACHE)).toBe(false);
		expect(ownCopy(`/other${CACHE}/ImagePicker/1F2E.jpg`, CACHE)).toBe(false);
	});

	it("never takes the folders themselves or a path that steps out of them", () => {
		expect(ownCopy(`${CACHE}/ImagePicker`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/ImagePicker/`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/ImagePicker/../maxhq/core.db`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/DocumentPicker/./a.pdf`, CACHE)).toBe(false);
		expect(ownCopy(`${CACHE}/DocumentPicker//a.pdf`, CACHE)).toBe(false);
	});

	it("takes nothing without a caches folder", () => {
		expect(ownCopy("/ImagePicker/1F2E.jpg", "")).toBe(false);
		expect(ownCopy("/ImagePicker/1F2E.jpg", "/")).toBe(false);
		expect(ownCopy("relative/ImagePicker/1F2E.jpg", "relative")).toBe(false);
	});
});

describe("discarder", () => {
	it("deletes a picker's copy once", () => {
		const removed: string[] = [];
		const discard = discarder(`${CACHE}/ImagePicker/1F2E.jpg`, CACHE, (path) =>
			removed.push(path)
		);
		discard?.();
		discard?.();
		expect(removed).toEqual([`${CACHE}/ImagePicker/1F2E.jpg`]);
	});

	it("gives nothing for a file the app didn't copy or one with no path", () => {
		const remove = () => {
			throw new Error("deleted");
		};
		expect(discarder("/var/mobile/Media/DCIM/IMG_0001.HEIC", CACHE, remove)).toBeUndefined();
		expect(discarder(null, CACHE, remove)).toBeUndefined();
	});
});
