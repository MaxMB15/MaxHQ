// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { mockSource } from "@/data/mock-source";
import { openOwnView } from "@/test/settings";
import { type Entity, FRESH_SESSIONS_SINCE } from "@maxhq/shared/bindings";
import type { DataSource } from "@maxhq/shared/data/source";
import { CURSOR, DEVBOX, MAC } from "@maxhq/shared/data/mock-view";
import { FixedNow } from "@maxhq/shared/lib/clock";
import { NOW } from "@maxhq/shared/test/build";
import { NO_PERMISSION } from "@maxhq/shared/tasks/draft";
import { createUiStore } from "@maxhq/shared/ui/store";

/** The app on the sample data, with each entity of the first snapshot changed by `edit`. */
const renderApp = (edit: (e: Entity) => Entity = (e) => e) => {
	const mock = mockSource({ now: () => NOW, tick: false });
	const source: DataSource = {
		...mock,
		snapshot: async () => {
			const snapshot = await mock.snapshot();
			return { ...snapshot, entities: snapshot.entities.map(edit) };
		},
	};
	render(
		<FixedNow value={NOW}>
			<App source={source} ui={createUiStore()} />
		</FixedNow>
	);
	return source;
};

const openTasks = async () => {
	await screen.findByRole("button", { name: "Settings" });
	await openOwnView("Scheduled tasks");
	await screen.findByRole("heading", { name: "Scheduled tasks" });
};

// A device's tasks, after picking it in the filter when the filter shows.
const section = (name: string) => {
	const filter = screen.queryByRole("group", { name: "Device" });
	const pick = filter ? within(filter).queryByRole("button", { name }) : null;
	if (pick && pick.getAttribute("aria-pressed") !== "true") fireEvent.click(pick);
	return within(screen.getByRole("region", { name }));
};

const editor = (name = "New scheduled task") => within(screen.getByRole("dialog", { name }));

// Opens a picker in the editor, or another part of the page, and presses one of its options.
const choose = async (name: string | ReturnType<typeof within>, label: string, text: string) => {
	const part = typeof name === "string" ? editor(name) : name;
	fireEvent.click(part.getByRole("combobox", { name: new RegExp(`^${label},`) }));
	const options = await screen.findAllByRole("option");
	const found = options.find((o) => o.textContent?.includes(text));
	if (!found) throw new Error(`${label} has no option "${text}"`);
	fireEvent.pointerDown(found, { pointerType: "mouse" });
	fireEvent.click(found);
};

describe("Settings › Scheduled tasks", () => {
	it("lists each device's tasks with when they run and how they went", async () => {
		renderApp();
		await openTasks();
		const mac = section("Mac");
		expect(mac.getByText("Morning summary")).toBeInTheDocument();
		expect(mac.getByText(/Weekdays at 09:00/)).toBeInTheDocument();
		expect(mac.getByText(/Check the release checklist/)).toBeInTheDocument();
		expect(mac.getByText(/Failed 18 min ago/)).toBeInTheDocument();
		expect(mac.getByText("Funnel needs setting up")).toBeInTheDocument();
		expect(mac.getByText(/Add the signing secret/)).toBeInTheDocument();
		const devbox = section("devbox");
		expect(devbox.getByText(/On pull requests in Example-User\/MaxHQ/)).toBeInTheDocument();
		expect(
			devbox.getByText("Instant through a webhook. Last delivery 12 min ago.")
		).toBeInTheDocument();
		expect(devbox.getByText("Funnel on")).toBeInTheDocument();
	});

	it("says why a task can't run now, with the page that fixes it", async () => {
		const reason =
			"The GitHub App can't manage this repo's webhooks, so this task checks every minute.";
		const link = "https://github.com/settings/apps/maxhq-example/permissions";
		renderApp((e) =>
			e.entity === "device_tasks" && e.device === DEVBOX
				? {
						...e,
						problems: [{ task: "task-reviews", reason, link }],
						webhooks: [{ task: "task-reviews", instant: false, last_delivery: null }],
					}
				: e
		);
		await openTasks();
		const devbox = section("devbox");
		expect(devbox.getByText(reason)).toBeInTheDocument();
		expect(
			devbox.getByRole("link", { name: "Open the GitHub App's settings" })
		).toHaveAttribute("href", link);
		expect(devbox.getByText("Checks every minute.")).toBeInTheDocument();
		expect(section("Mac").queryByText(reason)).not.toBeInTheDocument();
	});

	it("pauses a task and runs one now", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		const set = vi.spyOn(source, "setTaskEnabled");
		await openTasks();
		const mac = section("Mac");
		fireEvent.click(mac.getByRole("switch", { name: "Morning summary on" }));
		await waitFor(() =>
			expect(mac.getByRole("switch", { name: "Morning summary on" })).not.toBeChecked()
		);
		expect(set).toHaveBeenCalledWith("dev-mac", expect.any(String), false);
		// A pause sends nothing about the task but its ID, so a row a device
		// forged can't go back out as a whole task.
		expect(save).not.toHaveBeenCalled();

		fireEvent.click(mac.getByRole("button", { name: "Run Check the release checklist now" }));
		await waitFor(() => expect(mac.getByText(/Running now/)).toBeInTheDocument());
	});

	it("asks before a device's webhooks go on Funnel", async () => {
		const source = renderApp();
		const set = vi.spyOn(source, "setHookAddress");
		await openTasks();
		await choose(section("Mac"), "Reached", "This device only");
		expect(
			await section("Mac").findByText(/Only programs on Mac reach its webhooks/)
		).toBeInTheDocument();
		expect(
			section("Mac").getByText(/^http:\/\/127\.0\.0\.1:41234\/maxhq\/task-deploy\//)
		).toBeInTheDocument();

		await choose(section("Mac"), "Reached", "Tailscale Funnel");
		const ask = within(
			await screen.findByRole("dialog", { name: "Use Tailscale Funnel on Mac?" })
		);
		expect(
			ask.getByText(/leaves the rest of the device's Funnel and Serve setup alone/)
		).toBeInTheDocument();
		fireEvent.click(ask.getByRole("button", { name: "Cancel" }));
		await waitFor(() =>
			expect(
				screen.queryByRole("dialog", { name: "Use Tailscale Funnel on Mac?" })
			).toBeNull()
		);
		expect(set).toHaveBeenCalledTimes(1);

		await choose(section("Mac"), "Reached", "Tailscale Funnel");
		fireEvent.click(await screen.findByRole("button", { name: "Use Funnel" }));
		await waitFor(() => expect(set).toHaveBeenLastCalledWith("dev-mac", { kind: "funnel" }));
		expect(await section("Mac").findByText("Funnel needs setting up")).toBeInTheDocument();
		fireEvent.click(section("Mac").getByRole("button", { name: "Try again" }));
		await waitFor(() => expect(set).toHaveBeenCalledTimes(3));
	});

	it("moves a device's webhooks to your own address, with each token hidden", async () => {
		const source = renderApp();
		const set = vi.spyOn(source, "setHookAddress");
		await openTasks();
		const devbox = section("devbox");
		await choose(devbox, "Reached", "Your own address");
		expect(devbox.getByText(/points at http:\/\/127\.0\.0\.1:41234/)).toBeInTheDocument();
		fireEvent.change(devbox.getByRole("textbox", { name: "Address" }), {
			target: { value: "https://hooks.example.com" },
		});
		fireEvent.click(devbox.getByRole("button", { name: "Use address" }));
		expect(set).toHaveBeenCalledWith(DEVBOX, {
			kind: "own",
			base: "https://hooks.example.com",
		});
		expect(
			await devbox.findByText(/maxd can't check that it reaches port 41234/)
		).toBeInTheDocument();
		expect(
			devbox.getByText("https://hooks.example.com/maxhq/task-triage/••••••")
		).toBeInTheDocument();

		fireEvent.click(devbox.getByRole("button", { name: "Rotate URL" }));
		fireEvent.click(devbox.getByRole("button", { name: "Rotate" }));
		expect(
			await devbox.findByText(/^https:\/\/hooks\.example\.com\/maxhq\/task-triage\/[^•]+$/)
		).toBeInTheDocument();
	});

	it("hides the tasks of a device whose maxd is older", async () => {
		renderApp((e) =>
			e.entity === "device_link" && e.device === DEVBOX
				? {
						...e,
						capabilities: e.capabilities && {
							...e.capabilities,
							scheduled_tasks: false,
						},
					}
				: e
		);
		await openTasks();
		expect(screen.getByRole("region", { name: "Mac" })).toBeInTheDocument();
		expect(screen.queryByRole("region", { name: "devbox" })).toBeNull();
	});
});

describe("the task editor", () => {
	it("says why a task can't be saved", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));
		expect(await editor().findByText("Give it a name.")).toBeInTheDocument();
		fireEvent.change(editor().getByRole("textbox", { name: "Name" }), {
			target: { value: "Nightly" },
		});
		fireEvent.change(editor().getByRole("textbox", { name: "Prompt" }), {
			target: { value: "Run the tests" },
		});
		fireEvent.change(editor().getByRole("textbox", { name: "Every, minutes" }), {
			target: { value: "0" },
		});
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));
		expect(await editor().findByText("Run it every 1 minute or more.")).toBeInTheDocument();
	});

	it("makes a timed task that reaches an agent, on the agent's device", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		fireEvent.change(editor().getByRole("textbox", { name: "Name" }), {
			target: { value: "Nightly" },
		});
		fireEvent.change(editor().getByRole("textbox", { name: "Prompt" }), {
			target: { value: "Run the tests" },
		});
		await choose("New scheduled task", "Runs", "At");
		fireEvent.click(editor().getByRole("button", { name: "Sat" }));
		await choose("New scheduled task", "Sends to", "Release checklist");
		expect(editor().getByRole("combobox", { name: /^Lives on,/ })).toHaveTextContent("Mac");
		expect(editor().getByText(/doesn't wake up to run it/)).toBeInTheDocument();
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "New scheduled task" })).toBeNull()
		);
		expect(save).toHaveBeenCalledWith(
			"dev-mac",
			expect.objectContaining({
				name: "Nightly",
				trigger: expect.objectContaining({
					type: "at",
					days: ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday"],
				}),
				target: { type: "agent", device: "dev-mac", agent: "agent-release" },
				permission: NO_PERMISSION,
			})
		);
		expect(section("Mac").getByText("Nightly")).toBeInTheDocument();
	});

	it("starts the agent on a fresh session each run when ticked", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		fireEvent.change(editor().getByRole("textbox", { name: "Name" }), {
			target: { value: "Nightly" },
		});
		fireEvent.change(editor().getByRole("textbox", { name: "Prompt" }), {
			target: { value: "Run the tests" },
		});
		await choose("New scheduled task", "Sends to", "Release checklist");
		const fresh = editor().getByRole("checkbox", { name: "Start a fresh session each run" });
		expect(fresh).not.toBeChecked();
		fireEvent.click(fresh);
		expect(fresh).toBeChecked();
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));
		await waitFor(() =>
			expect(screen.queryByRole("dialog", { name: "New scheduled task" })).toBeNull()
		);
		expect(save.mock.calls[0]?.[1].target).toEqual({
			type: "agent",
			device: "dev-mac",
			agent: "agent-release",
			fresh: true,
		});
	});

	it("can't start a fresh session on an older maxd, and says to update it", async () => {
		renderApp((e) =>
			e.entity === "device_link" && e.device === MAC
				? { ...e, protocol: FRESH_SESSIONS_SINCE - 1 }
				: e
		);
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		await choose("New scheduled task", "Sends to", "Release checklist");
		expect(
			editor().getByRole("checkbox", { name: "Start a fresh session each run" })
		).toHaveAttribute("aria-disabled", "true");
		expect(editor().getByText(/runs an older maxd/)).toBeInTheDocument();
	});

	it("shows a new webhook's URL once, and sends the secret on its own", async () => {
		const source = renderApp();
		const secret = vi.spyOn(source, "setHookSecret");
		const on = vi.spyOn(source, "setHookOn");
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		fireEvent.change(editor().getByRole("textbox", { name: "Name" }), {
			target: { value: "Deploys" },
		});
		await choose("New scheduled task", "Runs", "Webhook");
		expect(editor().getByRole("textbox", { name: "Prompt" })).toHaveValue(
			"Handle this webhook:\n{{body}}"
		);
		fireEvent.click(editor().getByRole("checkbox", { name: "Check a signature" }));
		expect(editor().getByRole("textbox", { name: "Signature header" })).toHaveValue(
			"x-hub-signature-256"
		);
		fireEvent.change(editor().getByLabelText("Signing secret"), {
			target: { value: "s3cret" },
		});
		await choose("New scheduled task", "Sends to", "Release checklist");
		expect(editor().getByRole("combobox", { name: /^Lives on,/ })).toHaveTextContent("devbox");
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));

		const made = within(await screen.findByRole("dialog", { name: "Webhook made" }));
		expect(made.getByText(/^https:\/\/devbox\.tail1234\.ts\.net\/maxhq\//)).toBeInTheDocument();
		expect(made.getByRole("button", { name: "Copy URL" })).toBeInTheDocument();
		expect(secret).toHaveBeenCalledWith(expect.any(String), "s3cret");
		expect(on).toHaveBeenCalledWith(expect.any(String), true);
		fireEvent.click(made.getByRole("button", { name: "Done" }));

		fireEvent.click(await section("devbox").findByRole("button", { name: "Edit Deploys" }));
		const edit = editor("Edit task");
		expect(edit.queryByText(/^https:/)).toBeNull();
		expect(edit.getByText(/The URL showed once/)).toBeInTheDocument();
		expect(edit.getByLabelText("New signing secret")).toHaveValue("");
	});

	it("rotates a webhook's URL after asking", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(section("devbox").getByRole("button", { name: "Edit Triage new issues" }));
		const edit = editor("Edit task");
		fireEvent.click(edit.getByRole("button", { name: "Rotate URL" }));
		fireEvent.click(edit.getByRole("button", { name: "Rotate" }));
		expect(
			await edit.findByText(/^https:\/\/devbox\.tail1234\.ts\.net\/maxhq\/task-triage\//)
		).toBeInTheDocument();
	});

	it("shows what Funnel is missing, with the command to copy", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(section("Mac").getByRole("button", { name: "Edit Deploy preview" }));
		const edit = editor("Edit task");
		expect(edit.getByText("Funnel needs setting up on Mac")).toBeInTheDocument();
		expect(edit.getByText("sudo tailscale set --operator=$USER")).toBeInTheDocument();
		expect(edit.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
	});

	it("watches GitHub events from the App's repos, with the latency note", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(
			section("devbox").getByRole("button", { name: "Edit Review pull requests" })
		);
		const edit = editor("Edit task");
		expect(edit.getByRole("combobox", { name: /^Repository,/ })).toHaveTextContent(
			"Example-User/MaxHQ"
		);
		expect(edit.getByRole("checkbox", { name: "Pull requests" })).toBeChecked();
		expect(edit.getByRole("checkbox", { name: "Pushes" })).not.toBeChecked();
		expect(edit.getByText(/30 seconds to 6 hours/)).toBeInTheDocument();
		expect(edit.getByRole("textbox", { name: "Runs a day" })).toHaveValue("20");
	});

	it("keeps a new GitHub task to the repo's people, and says how to open it up", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(screen.getByRole("button", { name: "New task" }));
		fireEvent.change(editor().getByRole("textbox", { name: "Name" }), {
			target: { value: "Triage" },
		});
		await choose("New scheduled task", "Runs", "GitHub events");
		const rule = () =>
			editor().getByRole("checkbox", {
				name: "Only for the repo's owners, members and collaborators",
			});
		expect(rule()).toBeChecked();
		expect(editor().getByText(/any stranger can start a run/)).toBeInTheDocument();
		fireEvent.click(rule());
		expect(rule()).not.toBeChecked();
		fireEvent.click(rule());
		await choose("New scheduled task", "Repository", "Example-User/MaxHQ");
		fireEvent.click(editor().getByRole("checkbox", { name: "Issues" }));
		await choose("New scheduled task", "Sends to", "Release checklist");
		fireEvent.click(editor().getByRole("button", { name: "Make task" }));
		await waitFor(() => expect(save).toHaveBeenCalled());
		expect(save.mock.calls[0]?.[1].trigger).toMatchObject({
			type: "github_events",
			rules: { authors: ["OWNER", "MEMBER", "COLLABORATOR"], daily_runs: null },
		});
	});

	it("shows a GitHub task from before the rule with it off", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(
			section("devbox").getByRole("button", { name: "Edit Review pull requests" })
		);
		expect(
			editor("Edit task").getByRole("checkbox", {
				name: "Only for the repo's owners, members and collaborators",
			})
		).not.toBeChecked();
	});

	it("keeps a webhook's free text out once you pick the safe default, then lets one field in", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(section("devbox").getByRole("button", { name: "Edit Triage new issues" }));
		const edit = editor("Edit task");
		const fields = within(edit.getByRole("group", { name: "Fields in the prompt" }));
		expect(
			fields.getByText("This prompt fills in every field of a request")
		).toBeInTheDocument();
		expect(fields.getByText("body.issue.title")).toBeInTheDocument();
		expect(fields.getAllByText("Free text")).toHaveLength(2);
		expect(fields.queryByRole("checkbox")).toBeNull();

		fireEvent.click(fields.getByRole("button", { name: "Use the safe default" }));
		expect(fields.queryByText("This prompt fills in every field of a request")).toBeNull();
		expect(fields.getByRole("checkbox", { name: "Allow body.issue.body" })).not.toBeChecked();
		fireEvent.click(fields.getByRole("checkbox", { name: "Allow body.issue.title" }));
		fireEvent.click(edit.getByRole("button", { name: "Save task" }));
		await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit task" })).toBeNull());
		expect(save).toHaveBeenCalledWith(
			DEVBOX,
			expect.objectContaining({
				trigger: expect.objectContaining({
					rules: expect.objectContaining({ fields: ["body.issue.title"] }),
				}),
			})
		);
	});

	it("marks a GitHub event's structural fields, which need no allowing", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(
			section("devbox").getByRole("button", { name: "Edit Review pull requests" })
		);
		const fields = within(
			editor("Edit task").getByRole("group", { name: "Fields in the prompt" })
		);
		expect(fields.getByText("body.pull_request.html_url")).toBeInTheDocument();
		expect(fields.getByText("Structural")).toBeInTheDocument();
		expect(fields.queryByRole("checkbox")).toBeNull();
		expect(fields.queryByRole("status")).toBeNull();
	});

	it("waits for a quiet stretch, with the window restarting on each event", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(
			section("devbox").getByRole("button", { name: "Edit Review pull requests" })
		);
		const edit = editor("Edit task");
		fireEvent.change(edit.getByRole("textbox", { name: "Coalesce key" }), {
			target: { value: "all" },
		});
		fireEvent.change(edit.getByRole("textbox", { name: "Coalesce window, seconds" }), {
			target: { value: "900" },
		});
		expect(edit.queryByRole("textbox", { name: "Longest wait, seconds" })).toBeNull();
		fireEvent.click(
			edit.getByRole("checkbox", { name: "Restart the window with each request" })
		);
		expect(edit.getByRole("textbox", { name: "Longest wait, seconds" })).toHaveAttribute(
			"placeholder",
			"7200"
		);
		fireEvent.click(edit.getByRole("button", { name: "Save task" }));
		await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit task" })).toBeNull());
		expect(save).toHaveBeenCalledWith(
			DEVBOX,
			expect.objectContaining({
				trigger: expect.objectContaining({
					rules: expect.objectContaining({
						coalesce_key: "all",
						coalesce_secs: 900,
						coalesce_restart: true,
						coalesce_max_secs: null,
					}),
				}),
			})
		);
		expect(
			section("devbox").getByText(/Waits 15 min after the last event, 2 h at most/)
		).toBeInTheDocument();
	});

	it("keeps the agent a task reaches, though MaxHQ doesn't hold it", async () => {
		const source = renderApp((e) =>
			e.entity === "device_tasks" && e.device === DEVBOX
				? {
						...e,
						tasks: e.tasks.map((t) =>
							t.id === "task-reviews"
								? {
										...t,
										target: {
											type: "agent",
											device: DEVBOX,
											agent: "agent-gtfs",
										},
									}
								: t
						),
					}
				: e
		);
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(
			section("devbox").getByRole("button", { name: "Edit Review pull requests" })
		);
		const edit = editor("Edit task");
		expect(edit.getByRole("combobox", { name: /^Sends to,/ })).toHaveTextContent(
			"GTFS feed refresh"
		);
		fireEvent.click(edit.getByRole("button", { name: "Save task" }));
		await waitFor(() => expect(save).toHaveBeenCalled());
		expect(save).toHaveBeenCalledWith(
			DEVBOX,
			expect.objectContaining({
				target: { type: "agent", device: DEVBOX, agent: "agent-gtfs" },
			})
		);
	});

	it("starts a Cursor agent for each run, and shows it again", async () => {
		const source = renderApp();
		const save = vi.spyOn(source, "saveTask");
		await openTasks();
		fireEvent.click(section("Mac").getByRole("button", { name: "Edit Morning summary" }));
		await choose("Edit task", "Agent", "Cursor");
		fireEvent.click(editor("Edit task").getByRole("button", { name: "Save task" }));
		await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit task" })).toBeNull());
		expect(save).toHaveBeenCalledWith(
			MAC,
			expect.objectContaining({
				target: expect.objectContaining({
					type: "new",
					device: MAC,
					spec: expect.objectContaining({ vendor: "cursor", account: CURSOR }),
				}),
			})
		);
		fireEvent.click(section("Mac").getByRole("button", { name: "Edit Morning summary" }));
		expect(editor("Edit task").getByRole("combobox", { name: /^Agent,/ })).toHaveTextContent(
			"Cursor"
		);
	});

	it("deletes a task after asking", async () => {
		renderApp();
		await openTasks();
		fireEvent.click(section("Mac").getByRole("button", { name: "Edit Morning summary" }));
		fireEvent.click(editor("Edit task").getByRole("button", { name: "Delete" }));
		fireEvent.click(editor("Edit task").getByRole("button", { name: "Delete" }));
		await waitFor(() => expect(screen.queryByText("Morning summary")).toBeNull());
	});
});

describe("Deliveries and runs", () => {
	it("shows a task's requests and runs, and opens the agent a run reached", async () => {
		renderApp();
		await openTasks();
		const triage = section("devbox")
			.getByText("Triage new issues")
			.closest("div.flex.items-center") as HTMLElement;
		fireEvent.click(within(triage).getByRole("button", { name: "History" }));
		const dialog = within(await screen.findByRole("dialog", { name: "Triage new issues" }));
		const requests = within(await dialog.findByRole("list", { name: "Requests" }));
		expect(requests.getByText("Bad signature")).toBeInTheDocument();
		expect(requests.getByText("Duplicate")).toBeInTheDocument();

		fireEvent.click(dialog.getByRole("tab", { name: "Runs" }));
		const runs = within(await dialog.findByRole("list", { name: "Runs" }));
		fireEvent.click(runs.getByRole("button", { name: "GTFS feed refresh" }));
		await waitFor(() =>
			expect(
				screen.queryByRole("dialog", { name: "Use Tailscale Funnel on Mac?" })
			).toBeNull()
		);
		expect(
			screen.getByRole("heading", { level: 1, name: /GTFS feed refresh/ })
		).toBeInTheDocument();
	});
});

describe("Automations on an agent", () => {
	it("lists the tasks that reach the agent, with pause and Run now", async () => {
		const source = renderApp();
		const run = vi.spyOn(source, "runTaskNow");
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
		fireEvent.click(within(maxhq).getByRole("option", { name: /Release checklist/ }));
		const automations = within(await screen.findByRole("region", { name: "Automations" }));
		expect(automations.getByText("Check the release checklist")).toBeInTheDocument();
		expect(automations.getByRole("switch", { name: "Deploy preview on" })).not.toBeChecked();
		fireEvent.click(
			automations.getByRole("button", { name: "Run Check the release checklist now" })
		);
		await waitFor(() => expect(run).toHaveBeenCalledWith("task-checklist"));
		fireEvent.click(automations.getByRole("switch", { name: "Deploy preview on" }));
		await waitFor(() =>
			expect(automations.getByRole("switch", { name: "Deploy preview on" })).toBeChecked()
		);
	});

	it("runs a task once per press, says it started, and opens the task's editor from its name", async () => {
		const source = renderApp();
		let started: () => void = () => {};
		const run = vi.spyOn(source, "runTaskNow").mockImplementation(
			() =>
				new Promise((resolve) => {
					started = () => resolve({ status: "ok", data: { status: "done" } });
				})
		);
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
		fireEvent.click(within(maxhq).getByRole("option", { name: /Release checklist/ }));
		const automations = within(await screen.findByRole("region", { name: "Automations" }));
		const button = automations.getByRole("button", {
			name: "Run Check the release checklist now",
		});
		fireEvent.click(button);
		fireEvent.click(button);
		expect(button).toBeDisabled();
		expect(run).toHaveBeenCalledTimes(1);
		await act(async () => started());
		await waitFor(() => expect(button).toHaveTextContent("Started"));
		expect(button).toBeDisabled();
		expect(automations.getByRole("status")).toHaveTextContent(
			"Check the release checklist started."
		);

		fireEvent.click(automations.getByRole("button", { name: "Check the release checklist" }));
		const editor = await screen.findByRole("form", { name: "Edit task" });
		expect(within(editor).getByLabelText("Name")).toHaveValue("Check the release checklist");
	});

	it("shows nothing on an agent no task reaches", async () => {
		renderApp();
		const nav = screen.getByRole("navigation", { name: "Agents" });
		const maxhq = await within(nav).findByRole("region", { name: "MaxHQ" });
		fireEvent.click(within(maxhq).getByRole("option", { name: /Write the release notes/ }));
		await screen.findByRole("heading", { level: 1, name: /Write the release notes/ });
		expect(screen.queryByRole("region", { name: "Automations" })).toBeNull();
	});
});
