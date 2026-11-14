// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// Push on the phone: gives the token to the core, tells the core which
// agent is on screen, and opens or answers what a push is about.
// Allow and Deny answer in place, once Face ID unlocks the phone, through
// the same first-answer-wins path as the card. iOS wakes the app in the
// background for them, or starts it there when it wasn't running, so the
// links come back up for the answer and go again after. One whose answer
// can't reach the device leaves a note to open MaxHQ instead.
// Notifications for the agent you open, and for prompts answered since,
// leave Notification Center.

import { t, useI18n } from "@maxhq/shared/i18n";

import NetInfo from "@react-native-community/netinfo";
import { createNavigationContainerRef } from "@react-navigation/native";
import Constants from "expo-constants";
import * as Notifications from "expo-notifications";
import { useEffect } from "react";
import { AppState } from "react-native";

import type { AgentId } from "@maxhq/shared/bindings";
import {
	DENY_ACTION,
	denyFromButton,
	handledNotifications,
	heardLive,
	PERMISSION_CATEGORY,
	type Presented,
	pushEnvironment,
	type PushTarget,
	pushTarget,
	pushToken,
	targetPayload,
	UNREACHED_TITLE,
} from "@maxhq/shared/push/push";
import { phoneNetworkOf } from "@maxhq/shared/status/phone-network";
import { SYNC_GRACE_MS } from "@maxhq/shared/status/reach";

import type { Screens } from "../navigation";
import type { PhoneCalls } from "../data/native-source";
import { type Services, useServices } from "../services";
import { withBackgroundTime } from "./background";
import { pushStore } from "./kinds";
import { checkPermission, pushTopic } from "./permission";

/** The app's navigation, so a push can open its agent from outside a screen. */
export const pushNavigation = createNavigationContainerRef<Screens>();

const READY_TRIES = 50;
const READY_EVERY_MS = 100;

/** Opens the agent once the navigation is up, as after a cold start. */
const openAgent = (target: PushTarget, tries = READY_TRIES) => {
	if (pushNavigation.isReady()) {
		const { agent, attention } = target;
		pushNavigation.navigate("Agent", attention ? { agent, attention } : { agent });
	} else if (tries > 0) {
		setTimeout(() => openAgent(target, tries - 1), READY_EVERY_MS);
	}
};

/** What a notification carries: a push's user info, or a local note's data. */
const payloadOf = (notification: Notifications.Notification): unknown => {
	const { trigger, content } = notification.request;
	const push =
		trigger && "type" in trigger && trigger.type === "push" ? trigger.payload : undefined;
	return push ?? content.data;
};

/** Dials the links again while the app is in the background, as a new network does. */
const wakeLinks = async (phone: PhoneCalls) => {
	const kind = phoneNetworkOf(await NetInfo.fetch());
	if (kind) phone.networkChanged(kind);
};

const denyInPlace = async (services: Services, target: PushTarget) => {
	const { view, source, phone } = services;
	if (phone && AppState.currentState !== "active") await wakeLinks(phone);
	const inView = () =>
		target.attention
			? view.getState().view.tables.attention.get(target.attention)?.options
			: undefined;
	const outcome = await denyFromButton(target, inView, (id, answer) => source.answer(id, answer));
	if (outcome === "unreached") {
		await Notifications.scheduleNotificationAsync({
			content: { title: t(UNREACHED_TITLE), data: targetPayload(target) },
			trigger: null,
		});
	}
	// Lets the links go again, as when the app leaves the screen.
	if (phone && AppState.currentState !== "active") phone.appState("background");
};

// A response can come both from the listener and as the last response
// after a reload, so each is handled once.
const responded = new Set<string>();

const respond = async (services: Services, response: Notifications.NotificationResponse) => {
	const once = `${response.notification.request.identifier} ${response.actionIdentifier}`;
	if (responded.has(once)) return;
	responded.add(once);
	const target = pushTarget(payloadOf(response.notification));
	if (!target) return;
	if (response.actionIdentifier === DENY_ACTION) {
		await withBackgroundTime(() => denyInPlace(services, target));
	} else {
		openAgent(target);
	}
};

/** The agent on screen, if any. */
export const shownAgent = (): AgentId | null => {
	const route = pushNavigation.isReady() ? pushNavigation.getCurrentRoute() : undefined;
	return route?.name === "Agent" ? (route.params as Screens["Agent"]).agent : null;
};

const presentedNow = async (): Promise<Presented[]> =>
	(await Notifications.getPresentedNotificationsAsync()).map((notification) => ({
		id: notification.request.identifier,
		target: pushTarget(payloadOf(notification)),
	}));

/**
 * Clears the notifications that are done with while the app is on screen:
 * the ones for the agent you're looking at, and prompts answered since. It
 * looks as you move between screens and come back to the app, and a while
 * after the view changes, once the devices have had time to sync.
 */
const clearHandled = (services: Services): (() => void) => {
	let timer: ReturnType<typeof setTimeout> | undefined;
	const clear = async () => {
		if (AppState.currentState !== "active") return;
		const presented = await presentedNow();
		const view = services.view.getState().view;
		const done = handledNotifications(presented, view, shownAgent(), Date.now());
		await Promise.all(done.map((id) => Notifications.dismissNotificationAsync(id)));
	};
	const run = () => {
		clear().catch((error: unknown) => console.warn("Couldn't clear notifications", error));
	};
	const later = () => {
		timer ??= setTimeout(() => {
			timer = undefined;
			run();
		}, SYNC_GRACE_MS);
	};
	const moved = pushNavigation.addListener("state", run);
	const changed = services.view.subscribe(later);
	const back = AppState.addEventListener("change", (state) => {
		if (state === "active") run();
	});
	return () => {
		clearTimeout(timer);
		moved();
		changed();
		back.remove();
	};
};

const setUp = async (services: Services): Promise<() => void> => {
	const { phone } = services;
	const stops: (() => void)[] = [];
	const topic = pushTopic();
	if (!phone || !topic) return () => {};

	// A prompt the view has buzzes and shows the app's own banner instead.
	Notifications.setNotificationHandler({
		handleNotification: async (notification) => {
			const live = heardLive(
				pushTarget(payloadOf(notification)),
				services.view.getState().view
			);
			return {
				shouldShowBanner: !live,
				shouldShowList: true,
				shouldPlaySound: !live,
				shouldSetBadge: false,
			};
		},
	});
	// Deny answers without opening the app, once the phone is unlocked.
	await Notifications.setNotificationCategoryAsync(PERMISSION_CATEGORY, [
		{
			identifier: DENY_ACTION,
			buttonTitle: t("Deny"),
			options: {
				opensAppToForeground: false,
				isAuthenticationRequired: true,
				isDestructive: true,
			},
		},
	]);
	stops.push(clearHandled(services));

	const tapped = Notifications.addNotificationResponseReceivedListener((response) => {
		void respond(services, response);
	});
	stops.push(() => tapped.remove());
	const first = Notifications.getLastNotificationResponse();
	if (first) {
		Notifications.clearLastNotificationResponse();
		void respond(services, first);
	}

	const environment = pushEnvironment(Constants.expoConfig?.extra?.appVariant);
	const send = () => {
		const { token, kinds } = pushStore.getState();
		if (token) phone.pushToken(pushToken(token, topic, environment, kinds));
	};
	stops.push(pushStore.subscribe(send));
	const renewed = Notifications.addPushTokenListener(({ data }) => {
		if (typeof data === "string") pushStore.setState({ token: data });
	});
	stops.push(() => renewed.remove());

	// The Paired step asks; here it reads again on each return, as from Settings.
	checkPermission();
	const back = AppState.addEventListener("change", (state) => {
		if (state === "active") checkPermission();
	});
	stops.push(() => back.remove());
	return () => {
		for (const stop of stops) stop();
	};
};

/** Tells the core which agent is on screen, so its device holds back that agent's pushes. */
const watchScreen = (services: Services): (() => void) => {
	const { phone } = services;
	if (!phone) return () => {};
	let shown: string | null = null;
	return pushNavigation.addListener("state", () => {
		const agent = shownAgent();
		if (agent === shown) return;
		shown = agent;
		phone.presence(agent);
	});
};

/** Mounted once inside the services, beside the navigation it opens. */
export const PushSetup = () => {
	const { language } = useI18n();
	const services = useServices();
	useEffect(() => {
		let stop = () => {};
		let gone = false;
		setUp(services).then(
			(stopped) => (gone ? stopped() : (stop = stopped)),
			(error: unknown) => console.warn("Couldn't set up pushes", error)
		);
		const unwatch = watchScreen(services);
		return () => {
			gone = true;
			stop();
			unwatch();
		};
	}, [services, language]);
	return null;
};
