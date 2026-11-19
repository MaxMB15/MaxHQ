// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// The photos and files the paperclip adds: from the photo library, a new
// photo from the camera, or any file from the system's file picker. Each
// comes back as a picked file the box stages, and nothing when you cancel.
// The pickers copy what you pick into the app's caches folder, so the box
// can upload it from there, or read its bytes only once it stages. The box
// deletes such a copy once it isn't needed, and never a file the app
// didn't copy, as picked-copies.ts tells.

import { t } from "@maxhq/shared/i18n";

import type { PickedFile } from "@maxhq/shared/composer/staging";
import * as DocumentPicker from "expo-document-picker";
import { File, Paths } from "expo-file-system";
import * as ImagePicker from "expo-image-picker";

import { pathOf, urlOf } from "../../data/native-source";
import { discarder } from "./picked-copies";

/** Where the paperclip takes a file from. */
export type PickFrom = "library" | "camera" | "files";

/** What a pick gave: the files, or why it couldn't open. */
export type Picked = { readonly files: PickedFile[] } | { readonly failure: string };

const lastPart = (uri: string): string => decodeURIComponent(uri.split("/").pop() ?? "");

// Deletes a picker's copy, which may have gone already with the rest of the cache.
const removeCopy = (path: string) => {
	try {
		const file = new File(urlOf(path));
		if (file.exists) file.delete();
	} catch (error) {
		console.warn("Couldn't delete a picked file's copy", error);
	}
};

const pickedOf = (asset: {
	readonly uri: string;
	readonly name: string;
	readonly mime: string;
	readonly size: number | null;
	readonly picture: boolean;
}): PickedFile => {
	const path = asset.uri.startsWith("file://") ? pathOf(asset.uri) : null;
	return {
		name: asset.name,
		mime: asset.mime,
		size: asset.size,
		preview: asset.picture ? asset.uri : null,
		path,
		read: () => new File(asset.uri).bytes(),
		discard: discarder(path, pathOf(Paths.cache.uri), removeCopy),
	};
};

const pictures = (result: ImagePicker.ImagePickerResult): PickedFile[] =>
	result.canceled
		? []
		: result.assets.map((asset) =>
				pickedOf({
					uri: asset.uri,
					name: asset.fileName || lastPart(asset.uri) || "photo.jpg",
					mime: asset.mimeType ?? "image/jpeg",
					size: asset.fileSize ?? null,
					picture: true,
				})
			);

const messageOf = (error: unknown): string =>
	error instanceof Error ? error.message : String(error);

/** Opens the picker `from` names and gives what you picked. */
export const pickFiles = async (from: PickFrom): Promise<Picked> => {
	try {
		switch (from) {
			case "library":
				return {
					files: pictures(
						await ImagePicker.launchImageLibraryAsync({
							mediaTypes: ["images"],
							allowsMultipleSelection: true,
						})
					),
				};
			case "camera": {
				const allowed = await ImagePicker.requestCameraPermissionsAsync();
				if (!allowed.granted)
					return { failure: t("Allow the camera in Settings to take a photo.") };
				return {
					files: pictures(
						await ImagePicker.launchCameraAsync({ mediaTypes: ["images"] })
					),
				};
			}
			case "files": {
				const result = await DocumentPicker.getDocumentAsync({
					multiple: true,
					copyToCacheDirectory: true,
				});
				if (result.canceled) return { files: [] };
				return {
					files: result.assets.map((asset) => {
						const mime = asset.mimeType ?? "application/octet-stream";
						return pickedOf({
							uri: asset.uri,
							name: asset.name || lastPart(asset.uri) || "file",
							mime,
							size: asset.size ?? null,
							picture: mime.startsWith("image/"),
						});
					}),
				};
			}
		}
	} catch (error) {
		return {
			failure: t("Couldn't open the picker: {{error}}", { error: messageOf(error) }),
		};
	}
};
