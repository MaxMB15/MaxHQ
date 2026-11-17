// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import "./i18n";

import { useI18n } from "@maxhq/shared/i18n";

import {
	DarkTheme,
	type LinkingOptions,
	NavigationContainer,
	type Theme,
} from "@react-navigation/native";
import { createNativeStackNavigator } from "@react-navigation/native-stack";
import { StatusBar } from "expo-status-bar";
import { useState } from "react";
import { Linking } from "react-native";
import { SafeAreaProvider } from "react-native-safe-area-context";

import { createSource } from "./data";
import { AgentInfoScreen } from "./features/agent/AgentInfoScreen";
import { AgentScreen } from "./features/agent/AgentScreen";
import { MoveScreen } from "./features/agent/MoveScreen";
import { RunningScreen } from "./features/agent/RunningScreen";
import { SubAgentsScreen } from "./features/agent/SubAgentsScreen";
import { PromptAlerts } from "./features/attention/PromptAlerts";
import { DeviceScreen } from "./features/devices/DeviceScreen";
import { DevicesScreen } from "./features/devices/DevicesScreen";
import { LicencesScreen } from "./features/devices/LicencesScreen";
import { DiagramRenderer } from "./features/diagrams/DiagramRenderer";
import { InboxScreen } from "./features/inbox/InboxScreen";
import { NewAgentScreen } from "./features/new-agent/NewAgentScreen";
import { NoteScreen } from "./features/notes/NoteScreen";
import { NotesScreen } from "./features/notes/NotesScreen";
import { PhoneScreen } from "./features/phone/PhoneScreen";
import { PullScreen, PullsScreen } from "./features/pulls";
import { scanLinkOf } from "./features/scan/code";
import { ScanScreen } from "./features/scan/ScanScreen";
import { DeviceSettingsScreen } from "./features/settings/DeviceSettingsScreen";
import { ProjectSettingsScreen } from "./features/settings/ProjectSettingsScreen";
import { SettingsPageScreen } from "./features/settings/SettingsPageScreen";
import { SignInScreen } from "./features/settings/SignInScreen";
import { SkillLibraryScreen, SkillProposalScreen, SkillsScreen } from "./features/skills";
import { TaskActivityScreen, TaskEditorScreen, TasksScreen } from "./features/tasks";
import { LeaseScreen } from "./features/test-hosts/LeaseScreen";
import { UsageScreen } from "./features/usage/UsageScreen";
import { FileScreen } from "./features/viewer/FileScreen";
import { VisualScreen } from "./features/visuals/VisualScreen";
import { NewWorkspaceScreen } from "./features/workspaces/NewWorkspaceScreen";
import {
	agentPageId,
	agentScreenId,
	deviceScreenId,
	fileScreenId,
	leaseScreenId,
	noteScreenId,
	pullScreenId,
	type Screens,
} from "./navigation";
import { pushNavigation, PushSetup } from "./push/PushSetup";
import { ServicesProvider } from "./services";
import { color } from "./theme";
import { PictureViewer } from "./ui/Picture";

const Stack = createNativeStackNavigator<Screens>();

const theme: Theme = {
	...DarkTheme,
	colors: {
		...DarkTheme.colors,
		primary: color.primary,
		background: color.background,
		card: color.background,
		text: color.foreground,
		border: color.border,
		notification: color.ready,
	},
};

// Each screen draws its own compact header, as the desktop's panes do. The
// edge swipe still goes back.
const SCREEN = {
	headerShown: false,
	contentStyle: { backgroundColor: color.background },
} as const;

// maxhq://agent/<id> opens an agent, and ?attention=<id> scrolls to that
// prompt. maxhq://file/<agent>?path=<path>&line=<n> opens a file on the
// agent's device at a line. maxhq://device/<link> opens a device's page.
// maxhq://pull/<device>/<n>?repo=<owner/repo> opens a PR, maxhq://task/<id>
// a task's history, and maxhq://task-editor?task=<id> its editor (no task
// makes a new one). maxhq://skills/<project> opens a project's skills,
// maxhq://skill-library the library, and maxhq://skill-proposal/<id> a
// proposal.
// maxhq://phone is the old name for Settings. A pairing code,
// maxhq:<code>, opens Scan with the code ready to pair.
// Whatever a link opens sits over the inbox, so Back has somewhere to go.
const linking: LinkingOptions<Screens> = {
	prefixes: ["maxhq://"],
	getInitialURL: async () => {
		const url = await Linking.getInitialURL();
		return url ? scanLinkOf(url) : url;
	},
	subscribe: (listener) => {
		const opened = Linking.addEventListener("url", ({ url }) => listener(scanLinkOf(url)));
		return () => opened.remove();
	},
	config: {
		initialRouteName: "Inbox",
		screens: {
			Inbox: "",
			Agent: "agent/:agent",
			Usage: "usage",
			Devices: "devices",
			Settings: { path: "settings", alias: ["phone"] },
			Scan: "scan",
			Notes: "notes/:project",
			Note: "note/:project/:note",
			File: { path: "file/:agent", parse: { line: Number } },
			Device: "device/:link",
			Licences: "licences/:device",
			NewWorkspace: "new-workspace/:device",
			Lease: "lease/:device/:lease",
			Pulls: "pulls",
			Pull: { path: "pull/:device/:number", parse: { number: Number } },
			Tasks: "tasks",
			TaskActivity: "task/:task",
			TaskEditor: "task-editor",
			SettingsPage: "settings/:page",
			SignIn: "sign-in/:login",
			DeviceSettings: "device-settings/:device",
			ProjectSettings: "project-settings/:project",
			AgentInfo: "agent-info/:agent",
			MoveAgent: "move-agent/:agent",
			Running: "running/:agent",
			SubAgents: "sub-agents/:agent",
			NewAgent: "new-agent",
			Skills: "skills/:project",
			SkillLibrary: "skill-library",
			SkillProposal: "skill-proposal/:proposal",
		},
	},
};

export const App = () => {
	useI18n();

	const [{ source, phone }] = useState(createSource);
	return (
		<SafeAreaProvider>
			<ServicesProvider source={source} phone={phone}>
				<PushSetup />
				<NavigationContainer ref={pushNavigation} theme={theme} linking={linking}>
					<Stack.Navigator screenOptions={SCREEN}>
						<Stack.Screen name="Inbox" component={InboxScreen} />
						<Stack.Screen name="Agent" component={AgentScreen} getId={agentScreenId} />
						<Stack.Screen
							name="AgentInfo"
							component={AgentInfoScreen}
							getId={agentPageId}
						/>
						<Stack.Screen
							name="Running"
							component={RunningScreen}
							getId={agentPageId}
						/>
						<Stack.Screen
							name="SubAgents"
							component={SubAgentsScreen}
							getId={agentPageId}
						/>
						<Stack.Screen
							name="MoveAgent"
							component={MoveScreen}
							options={{ presentation: "modal" }}
						/>
						<Stack.Screen
							name="NewAgent"
							component={NewAgentScreen}
							options={{ presentation: "modal" }}
						/>
						<Stack.Screen name="Usage" component={UsageScreen} />
						<Stack.Screen name="Devices" component={DevicesScreen} />
						<Stack.Screen name="Settings" component={PhoneScreen} />
						<Stack.Screen name="SettingsPage" component={SettingsPageScreen} />
						<Stack.Screen name="SignIn" component={SignInScreen} />
						<Stack.Screen name="DeviceSettings" component={DeviceSettingsScreen} />
						<Stack.Screen name="ProjectSettings" component={ProjectSettingsScreen} />
						<Stack.Screen name="Skills" component={SkillsScreen} />
						<Stack.Screen name="SkillLibrary" component={SkillLibraryScreen} />
						<Stack.Screen name="SkillProposal" component={SkillProposalScreen} />
						<Stack.Screen name="Notes" component={NotesScreen} />
						<Stack.Screen name="Note" component={NoteScreen} getId={noteScreenId} />
						<Stack.Screen name="File" component={FileScreen} getId={fileScreenId} />
						<Stack.Screen name="Pulls" component={PullsScreen} />
						<Stack.Screen name="Pull" component={PullScreen} getId={pullScreenId} />
						<Stack.Screen name="Tasks" component={TasksScreen} />
						<Stack.Screen name="TaskActivity" component={TaskActivityScreen} />
						<Stack.Screen
							name="TaskEditor"
							component={TaskEditorScreen}
							options={{ presentation: "modal" }}
						/>
						<Stack.Screen
							name="Device"
							component={DeviceScreen}
							getId={deviceScreenId}
						/>
						<Stack.Screen name="Lease" component={LeaseScreen} getId={leaseScreenId} />
						<Stack.Screen
							name="Licences"
							component={LicencesScreen}
							options={{ presentation: "modal" }}
						/>
						<Stack.Screen
							name="NewWorkspace"
							component={NewWorkspaceScreen}
							options={{ presentation: "modal" }}
						/>
						<Stack.Screen
							name="Scan"
							component={ScanScreen}
							options={{ presentation: "modal" }}
						/>
					</Stack.Navigator>
				</NavigationContainer>
				<PromptAlerts />
				<PictureViewer />
				<VisualScreen />
				<DiagramRenderer />
				<StatusBar style="light" />
			</ServicesProvider>
		</SafeAreaProvider>
	);
};
