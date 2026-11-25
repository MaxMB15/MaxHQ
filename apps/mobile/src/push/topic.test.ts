// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { describe, expect, it } from "vitest";

import { pushTopicOf } from "./topic";

const config = {
	ios: { bundleIdentifier: "dev.example.ios" },
	android: { package: "dev.example.android" },
};

describe("pushTopicOf", () => {
	it("is the bundle id on a build signed for push", () => {
		expect(pushTopicOf({ ...config, extra: { push: true } }, "apns")).toBe("dev.example.ios");
	});

	it("is the package name on a build with Firebase's file", () => {
		expect(pushTopicOf({ ...config, extra: { fcm: true } }, "fcm")).toBe("dev.example.android");
	});

	it("is null where the build gets no pushes through that service", () => {
		expect(pushTopicOf({ ...config, extra: { fcm: true } }, "apns")).toBeNull();
		expect(pushTopicOf({ ...config, extra: { push: true } }, "fcm")).toBeNull();
		expect(pushTopicOf({ ...config, extra: { push: "yes", fcm: 1 } }, "apns")).toBeNull();
		expect(pushTopicOf({ ...config, extra: null }, "fcm")).toBeNull();
		expect(pushTopicOf(null, "apns")).toBeNull();
		expect(pushTopicOf({ extra: { fcm: true } }, "fcm")).toBeNull();
	});
});
