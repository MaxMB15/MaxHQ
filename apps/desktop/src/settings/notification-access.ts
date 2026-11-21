// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What macOS allows MaxHQ's notifications, for the Notifications and Badge
// cards. It reads again whenever the window comes back to the front, since
// that's where it changes, in System Settings.

import { useCallback, useEffect, useState } from "react";

import { type NotificationAccess, notificationAccess } from "@/native/notifications";

/** What macOS allows, null until read, and a way to read it again. */
export const useNotificationAccess = (): {
	readonly access: NotificationAccess | null;
	readonly check: () => void;
} => {
	const [access, setAccess] = useState<NotificationAccess | null>(null);
	const check = useCallback(() => {
		void notificationAccess()
			.then(setAccess)
			.catch(() => setAccess(null));
	}, []);
	useEffect(() => {
		check();
		window.addEventListener("focus", check);
		return () => window.removeEventListener("focus", check);
	}, [check]);
	return { access, check };
};
