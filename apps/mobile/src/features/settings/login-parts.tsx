// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

// What the phone's login pages share, as the desktop's do: a login's steps,
// the devices to tick, the pasted secret and its send, how each device took
// it, what each device holds with Forget, and Sign in again for the logins
// a config folder holds. Sending, forgetting and signing in are requests a
// device takes from the phone only with the Mac's switch on for it, so a
// device without it can't be ticked, its buttons stay off, and the line
// under them says where the switch is.

import { useI18n } from "@maxhq/shared/i18n";

import * as Clipboard from "expo-clipboard";
import { type ComponentRef, type ReactNode, useCallback, useRef, useState } from "react";
import { Linking, StyleSheet, Text, View } from "react-native";

import type { Account, CredentialKind, DeviceId, Timestamp, Vendor } from "@maxhq/shared/bindings";
import type { ActionResult } from "@maxhq/shared/data/source";
import { copyLabel, useCopyWith } from "@maxhq/shared/lib/copy";
import { cliLoginBlocked, loggedOut } from "@maxhq/shared/settings/cli-sign-in";
import { forgetNote, type Outcome, outcomesOf } from "@maxhq/shared/settings/login-parts";
import { LOGIN_STEPS, type LoginPage, stepParts } from "@maxhq/shared/settings/login-steps";
import {
	deviceFirst,
	folderAccounts,
	type Held,
	type Target,
	usable,
	withoutWhitespace,
} from "@maxhq/shared/settings/logins";
import { letsPhone } from "@maxhq/shared/settings/phones";
import { formatAgo, sentence } from "@maxhq/shared/status/format";
import { scrub } from "@maxhq/shared/data/failure";
import type { ViewState } from "@maxhq/shared/view/replica";
import { deviceName } from "@maxhq/shared/view/select";

import { useServices, useView } from "../../services";
import { color, font, size } from "../../theme";
import { Button } from "../../ui/Button";
import { Card } from "../../ui/Card";
import { confirm } from "../../ui/choose";
import { Input } from "../../ui/Field";
import { opensInBrowser } from "../../ui/markdown";
import { SignInSheet } from "../agent/SignInAgain";
import { Buttons, CardBody, Muted, type Note, NoteLine } from "../devices/section";
import { NeedsSwitch } from "../phone/everything";
import { GatedToggle } from "../test-hosts/parts";

/** Opens a page in the browser, when it's one the phone opens. */
export const openPage = (url: string) => {
	if (opensInBrowser(url)) Linking.openURL(url).catch(() => {});
};

/**
 * Whether each device takes this phone's gated requests, as `useLetsPhone`
 * says for one device, for the pages that list several. Off the phone's
 * core, such as with the sample data, every device does.
 */
export const useLets = (): ((device: DeviceId) => boolean) => {
	const { phone } = useServices();
	const view = useView((s) => s.view);
	const [id] = useState(() => {
		const key = phone?.phoneKey();
		return key?.status === "ok" ? key.data.phone : null;
	});
	return (device) => phone === null || (id !== null && letsPhone(view, id, device));
};

/**
 * The targets with each one the phone may not send to marked, so it can't
 * be ticked, and whether any is.
 */
export const useGated = (targets: readonly Target[]): { targets: Target[]; someOff: boolean } => {
	const { t } = useI18n();
	const lets = useLets();
	const gated = targets.map((target) =>
		lets(target.device)
			? target
			: { ...target, blocked: target.blocked ?? t("Not from this phone") }
	);
	return { targets: gated, someOff: targets.some((target) => !lets(target.device)) };
};

/** A step's text, with its commands set in a mono font, links to open and values to copy. */
const StepText = ({
	text,
	onCopy,
	muted,
}: {
	text: string;
	onCopy: (text: string) => void;
	muted?: boolean;
}) => {
	const { t } = useI18n();
	return (
		<Text style={[styles.step, muted && styles.meta]}>
			{stepParts(text).map((part, i) => {
				switch (part.kind) {
					case "text":
						return part.text;
					case "link":
						return (
							<Text
								key={i}
								accessibilityRole="link"
								accessibilityHint={t(
									"Opens in the browser. Touch and hold to copy."
								)}
								onPress={() => openPage(part.text)}
								onLongPress={() => onCopy(part.text)}
								style={styles.link}
							>
								{part.text}
							</Text>
						);
					case "copy":
						return (
							<Text
								key={i}
								accessibilityRole="button"
								accessibilityHint={t("Copies it")}
								onPress={() => onCopy(part.text)}
								style={[styles.code, styles.copy]}
							>
								{part.text}
							</Text>
						);
					default:
						return (
							<Text key={i} style={styles.code}>
								{part.text}
							</Text>
						);
				}
			})}
		</Text>
	);
};

/** A muted line written as a step is, such as one that names a command. */
export const StepNote = ({ text }: { text: string }) => {
	const [, copy] = useCopyWith(Clipboard.setStringAsync);
	return <StepText text={text} onCopy={copy} muted />;
};

/** A login's steps as a numbered list, with the notes before and after. */
export const StepsCard = ({ login }: { login: LoginPage }) => {
	const { t } = useI18n();
	const [copied, copy] = useCopyWith(Clipboard.setStringAsync);
	const [notesOpen, setNotesOpen] = useState(false);
	const { title, before, steps, after } = LOGIN_STEPS[login];
	const notes = [...before, ...after];
	return (
		<Card
			title={t("Steps")}
			note={t("Once for each login")}
			label={t("Steps for {{title}}", { title: title })}
		>
			<CardBody first>
				{steps.map((step, i) => (
					<View key={i} style={styles.stepRow}>
						<Text style={styles.number}>{i + 1}</Text>
						<View style={styles.stepBody}>
							<StepText text={step} onCopy={copy} />
						</View>
					</View>
				))}
				{copied === "idle" ? null : (
					<Text accessibilityLiveRegion="polite" style={styles.meta}>
						{copyLabel(copied, "")}
					</Text>
				)}
			</CardBody>
			{notes.length > 0 ? (
				<CardBody>
					<Button
						label={t("How MaxHQ keeps it")}
						tone="link"
						small
						onPress={() => setNotesOpen((open) => !open)}
					/>
					{notesOpen
						? notes.map((note, i) => <StepText key={i} text={note} onCopy={copy} />)
						: null}
				</CardBody>
			) : null}
		</Card>
	);
};

/** The devices to send a login to, each with a switch. */
export const DeviceTicks = ({
	targets,
	ticked,
	onChange,
	disabled,
	label,
	empty,
	away,
}: {
	targets: readonly Target[];
	ticked: ReadonlySet<DeviceId>;
	onChange: (ticked: ReadonlySet<DeviceId>) => void;
	disabled?: boolean;
	/** What the list is called. */
	label?: string;
	/** What shows when there's no device to send to. */
	empty?: string;
	/** What a device that isn't live says under its name, if anything. */
	away?: string | null;
}) => {
	const { t } = useI18n();
	const awayText = away === undefined ? t("Not connected") : away;
	return (
		<View accessibilityLabel={t("Devices")} style={styles.ticks}>
			<Text style={styles.label}>{label ?? t("Devices that host workspaces")}</Text>
			{targets.length === 0 ? (
				<Muted>
					{empty ?? t("No device has Incus yet, so there's nowhere to send it.")}
				</Muted>
			) : (
				targets.map((target) => {
					const sub = !target.live ? awayText : (target.blocked ?? null);
					return (
						<View key={target.device} style={styles.tick}>
							<View style={styles.tickText}>
								<Text style={[styles.name, !usable(target) && styles.dim]}>
									{target.name}
								</Text>
								{sub ? <Text style={styles.meta}>{sub}</Text> : null}
							</View>
							<GatedToggle
								label={target.name}
								value={ticked.has(target.device)}
								disabled={Boolean(disabled) || !usable(target)}
								onChange={(on) => {
									const next = new Set(ticked);
									if (on) next.add(target.device);
									else next.delete(target.device);
									onChange(next);
								}}
							/>
						</View>
					);
				})
			)}
		</View>
	);
};

/** One line for each device a login went to. */
export const Outcomes = ({
	view,
	outcomes,
	done,
}: {
	view: ViewState;
	outcomes: readonly Outcome[];
	/** What a device that took it shows, such as the repos it reaches. */
	done: (device: DeviceId) => string;
}) => {
	const { t } = useI18n();
	if (outcomes.length === 0) return null;
	return (
		<View accessibilityLabel={t("Results")} style={styles.outcomes}>
			{outcomes.map((o) => (
				<View key={o.device}>
					<Text style={[styles.outcome, o.failure ? styles.error : styles.ready]}>
						{o.failure
							? `${deviceName(view, o.device)}: ${sentence(o.failure)}.`
							: t("{{deviceName}} has it.", {
									deviceName: deviceName(view, o.device),
								})}
					</Text>
					{o.failure ? null : <Text style={styles.meta}>{done(o.device)}</Text>}
				</View>
			))}
		</View>
	);
};

/** A secret pasted into a field and sent to devices, with how it went. */
export type SecretSend = {
	/** Attaches the field, which only Send reads and clears. */
	readonly field: (el: ComponentRef<typeof Input> | null) => void;
	readonly onChange: (text: string) => void;
	/** Whether the field holds something, never what. */
	readonly typed: boolean;
	readonly busy: boolean;
	readonly outcomes: readonly Outcome[];
	readonly failure: string | null;
	/** Reads and clears the field, then sends what it held to `devices`. */
	readonly send: (devices: DeviceId[]) => Promise<void>;
};

/**
 * The state behind a pasted secret. What you paste sits in a ref until Send
 * reads it and hands it to `deliver`; no state, store or log keeps it, and
 * a reply that echoes it shows `<secret>`.
 */
export const useSecretSend = ({
	deliver,
	what,
	retry,
}: {
	deliver: (devices: DeviceId[], secret: string) => Promise<ActionResult[]>;
	/** What the secret is, for the console. */
	what: string;
	/** What shows when the send fails as a whole. */
	retry: string;
}): SecretSend => {
	const { t } = useI18n();
	const field = useRef<ComponentRef<typeof Input>>(null);
	const value = useRef("");
	const [typed, setTyped] = useState(false);
	const [busy, setBusy] = useState(false);
	const [outcomes, setOutcomes] = useState<Outcome[]>([]);
	const [failure, setFailure] = useState<string | null>(null);

	const onChange = (text: string) => {
		value.current = text;
		setTyped(text.trim() !== "");
	};

	const send = async (devices: DeviceId[]) => {
		if (busy) return;
		const secret = withoutWhitespace(value.current);
		value.current = "";
		field.current?.clear();
		setTyped(false);
		if (!secret) return;
		setBusy(true);
		setFailure(null);
		setOutcomes([]);
		try {
			setOutcomes(outcomesOf(devices, await deliver(devices, secret), secret));
		} catch (error) {
			const reason = error instanceof Error ? error.message : String(error);
			console.warn(t("Couldn't send the {{what}}:", { what: what }), scrub(reason, secret));
			setFailure(retry);
		} finally {
			setBusy(false);
		}
	};

	const attach = useCallback((el: ComponentRef<typeof Input> | null) => {
		field.current = el;
	}, []);

	return { field: attach, onChange, typed, busy, outcomes, failure, send };
};

/** The field a secret is pasted into, which shows dots for what it holds. */
export const SecretField = ({
	field,
	onChange,
	busy,
	label,
	placeholder,
}: {
	field: SecretSend["field"];
	onChange: SecretSend["onChange"];
	busy: boolean;
	label: string;
	placeholder: string;
}) => (
	<View style={styles.fieldGroup}>
		<Text style={styles.label}>{label}</Text>
		<Input
			ref={field}
			secureTextEntry
			accessibilityLabel={label}
			placeholder={placeholder}
			autoCapitalize="none"
			autoCorrect={false}
			autoComplete="off"
			spellCheck={false}
			editable={!busy}
			onChangeText={onChange}
			style={styles.mono}
		/>
	</View>
);

/** A plain field with its label over it. */
export const TextRow = ({
	label,
	value,
	placeholder,
	disabled,
	mono,
	onChange,
}: {
	label: string;
	value: string;
	placeholder?: string;
	disabled?: boolean;
	mono?: boolean;
	onChange: (text: string) => void;
}) => (
	<View style={styles.fieldGroup}>
		<Text style={styles.label}>{label}</Text>
		<Input
			accessibilityLabel={label}
			value={value}
			placeholder={placeholder}
			autoCapitalize="none"
			autoCorrect={false}
			autoComplete="off"
			spellCheck={false}
			editable={!disabled}
			onChangeText={onChange}
			style={[mono && styles.mono, disabled && styles.dim]}
		/>
	</View>
);

/** How the send went, then the Send button, and the switch's line when it keeps any device off. */
export const SendFooter = ({
	view,
	secret,
	ready,
	someOff,
	devices,
	done,
}: {
	view: ViewState;
	secret: SecretSend;
	/** Whether everything Send needs is there. */
	ready: boolean;
	/** Whether the Mac's switch keeps any of the devices off. */
	someOff: boolean;
	devices: DeviceId[];
	done: (device: DeviceId) => string;
}) => {
	const { t } = useI18n();
	return (
		<View style={styles.footer}>
			{secret.failure ? (
				<NoteLine note={{ text: secret.failure, error: true }} />
			) : (
				<Outcomes view={view} outcomes={secret.outcomes} done={done} />
			)}
			<Buttons>
				<Button
					label={secret.busy ? t("Sending") : t("Send")}
					tone="primary"
					disabled={!ready || !secret.typed || secret.busy}
					onPress={() => void secret.send(devices)}
				/>
			</Buttons>
			{someOff ? <NeedsSwitch /> : null}
		</View>
	);
};

/** What the button that drops a login says, and its note while it runs. */
export type ForgetWords = { readonly label: string; readonly busy: string };

/** Forget's words, read when shown so they follow the language. */
export const useForgetWords = (): ForgetWords => {
	const { t } = useI18n();
	return { label: t("Forget"), busy: t("Forgetting") };
};

/** Sign out's words, for a login that ends a session too. */
export const useSignOutWords = (): ForgetWords => {
	const { t } = useI18n();
	return { label: t("Sign out"), busy: t("Signing out") };
};

/** The button that drops a login a device holds, with a confirm step and how it went. */
export const Forget = ({
	device,
	credential,
	what,
	live,
	lets,
	words: { label, busy },
}: {
	device: DeviceId;
	credential: string;
	what: string;
	live: boolean;
	/** Whether the Mac lets this phone ask the device. */
	lets: boolean;
	words: ForgetWords;
}) => {
	const { t } = useI18n();
	const { source } = useServices();
	const [note, setNote] = useState<Note | null>(null);
	const forget = async () => {
		setNote({ text: busy, error: false });
		try {
			setNote(forgetNote(await source.forgetCredential(device, credential)));
		} catch (error) {
			console.warn(t("Couldn't forget the login"), error);
			setNote({ text: t("Couldn't reach the device. Try again."), error: true });
		}
	};
	return (
		<>
			<Button
				label={label}
				tone="danger"
				small
				disabled={!live || !lets}
				onPress={() =>
					confirm({ title: `${label} ${what}?`, detail: "" }, label, () => void forget())
				}
			/>
			<NoteLine note={note} />
		</>
	);
};

/** What the devices hold of one kind, each with Forget. */
export const HeldList = <T extends CredentialKind["type"]>({
	view,
	held,
	now,
	live,
	what,
	describe,
	actions,
	empty,
	forget,
}: {
	view: ViewState;
	held: readonly Held<T>[];
	now: Timestamp;
	live: (device: DeviceId) => boolean;
	/** What Forget's question calls it, such as "the token". */
	what: string;
	/** The lines under the device's name. */
	describe: (credential: Held<T>) => string | readonly string[];
	/** Buttons before Forget, such as Check again. */
	actions?: (credential: Held<T>) => ReactNode;
	empty: string;
	/** What the button that drops it says, such as Sign out. */
	forget?: ForgetWords;
}) => {
	const { t } = useI18n();
	const lets = useLets();
	const forgetWords = useForgetWords();
	const words = forget ?? forgetWords;
	if (held.length === 0) {
		return (
			<CardBody first>
				<Muted>{empty}</Muted>
			</CardBody>
		);
	}
	let off = false;
	const rows = held.map((c, i) => {
		const name = deviceName(view, c.device);
		const allowed = lets(c.device);
		off ||= !allowed;
		const lines = describe(c);
		return (
			<CardBody key={c.id} first={i === 0}>
				<View accessibilityLabel={t("On {{deviceName}}", { deviceName: name })}>
					<Text style={styles.name}>{name}</Text>
					{(typeof lines === "string" ? [lines] : lines).map((line) => (
						<Text key={line} style={styles.meta}>
							{line}
						</Text>
					))}
					<Text style={styles.meta}>
						{t("Stored {{ago}}", { ago: formatAgo(c.stored_at, now) })}
					</Text>
					{c.problem ? (
						<Text style={[styles.meta, styles.error]}>{`${sentence(c.problem)}.`}</Text>
					) : null}
				</View>
				<Buttons>
					{actions?.(c)}
					<Forget
						device={c.device}
						credential={c.id}
						what={t("{{what}} on {{deviceName}}", { what: what, deviceName: name })}
						live={live(c.device)}
						lets={allowed}
						words={words}
					/>
				</Buttons>
			</CardBody>
		);
	});
	return (
		<>
			{rows}
			{off ? (
				<CardBody>
					<NeedsSwitch />
				</CardBody>
			) : null}
		</>
	);
};

/** One login in a config folder, with the button that runs its CLI's login. */
const SignInRow = ({
	view,
	account,
	first,
	lets,
	onSignIn,
}: {
	view: ViewState;
	account: Account;
	first: boolean;
	lets: boolean;
	onSignIn: () => void;
}) => {
	const { t } = useI18n();
	const name = deviceName(view, account.device);
	const blocked = cliLoginBlocked(view, account);
	const failure = loggedOut(account) ? account.usage_failure : null;
	const action = failure ? t("Sign in") : t("Sign in again");
	const sub = blocked ?? account.email;
	return (
		<CardBody first={first}>
			<Text style={styles.name}>
				{t("{{label}} on {{name}}", { label: account.label, name: name })}
			</Text>
			{sub ? <Text style={styles.meta}>{sub}</Text> : null}
			<NoteLine note={failure ? { text: sentence(failure.reason), error: true } : null} />
			<Buttons>
				<Button
					label={action}
					small
					hint={t("{{action}} to {{label}} on {{name}}", {
						action: action,
						label: account.label,
						name: name,
					})}
					disabled={blocked !== null || !lets}
					onPress={onSignIn}
				/>
			</Buttons>
		</CardBody>
	);
};

/**
 * Sign in again for each login of `vendor` a config folder holds, which
 * runs its CLI's login on the device that holds it, in a sheet. `device`
 * puts that device's logins first.
 */
export const SignInAgainCard = ({ vendor, device }: { vendor: Vendor; device?: DeviceId }) => {
	const { t } = useI18n();
	const view = useView((s) => s.view);
	const lets = useLets();
	const [open, setOpen] = useState<Account["id"] | null>(null);
	const accounts = deviceFirst(folderAccounts(view, vendor), device);
	if (accounts.length === 0) return null;
	const off = accounts.some((a) => !lets(a.device));
	return (
		<Card title={t("Sign in again")} note={t("Runs the login on the device that holds it")}>
			{accounts.map((account, i) => (
				<SignInRow
					key={account.id}
					view={view}
					account={account}
					first={i === 0}
					lets={lets(account.device)}
					onSignIn={() => setOpen(account.id)}
				/>
			))}
			{off ? (
				<CardBody>
					<NeedsSwitch />
				</CardBody>
			) : null}
			{open !== null ? <SignInSheet id={open} close={() => setOpen(null)} /> : null}
		</Card>
	);
};

const styles = StyleSheet.create({
	step: { color: color.foreground, fontSize: size.small, lineHeight: 20 },
	stepRow: { flexDirection: "row", gap: 10 },
	stepBody: { flex: 1, minWidth: 0 },
	number: {
		width: 20,
		height: 20,
		borderRadius: 10,
		overflow: "hidden",
		textAlign: "center",
		lineHeight: 20,
		backgroundColor: color.secondary,
		color: color.secondaryForeground,
		fontSize: size.pill,
		fontWeight: "600",
		fontVariant: ["tabular-nums"],
	},
	code: { fontFamily: font.mono, fontSize: size.meta, color: color.foreground },
	copy: { color: color.primary },
	link: { color: color.primary },
	label: { color: color.secondaryForeground, fontSize: size.meta, fontWeight: "600" },
	ticks: { gap: 2 },
	tick: { flexDirection: "row", alignItems: "center", gap: 10 },
	tickText: { flex: 1, minWidth: 0 },
	name: { color: color.foreground, fontSize: size.body },
	meta: { color: color.mutedForeground, fontSize: size.meta },
	dim: { opacity: 0.5 },
	outcomes: { gap: 4 },
	outcome: { fontSize: size.meta, fontWeight: "600" },
	error: { color: color.errorText },
	ready: { color: color.readyText },
	fieldGroup: { gap: 6 },
	mono: { fontFamily: font.mono, fontSize: size.meta },
	footer: { gap: 8 },
});
