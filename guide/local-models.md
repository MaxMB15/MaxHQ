<!--
SPDX-License-Identifier: MIT
Copyright (c) 2026 Max Boksem
-->

# Local models

Local agents run on a model on your own hardware or on a server you pick, not on a cloud account. A cloud service such as OpenRouter runs them too, under its own name. This page covers starting one, what it shows while it works, and the Local models page in Settings.

## Local agents

<!-- id: local-models/agents -->

A Local agent has no sign-in, no quota and no usage meter. The usage popover leaves it out, apart from the credits of a cloud server such as OpenRouter; see Credits on OpenRouter below.

1. Open New agent and pick Local under Agent.
2. Pick its model under Model. Models are grouped by the server that runs them, with This Mac for the models MaxHQ runs on the Mac you're on. Each one says whether it fits the device. A model you haven't downloaded yet says Not downloaded, and picking it opens Settings to download it.
3. Start the agent as you would any other.

When no model is ready, Local still shows, with what to do first, such as "Download a model in Settings first" or "Add a server URL in Settings first". Set up local models… opens the Local models page.

To move an agent to a local model, open its Move menu and pick Move to Local and the server, such as Move to Local, This Mac.

## While a model loads

<!-- id: local-models/loading -->

A model that isn't in memory yet loads on the agent's first turn, and the turn waits for it.

- Loading model shows above the composer with the model's name, and a bar once the server says how far it got. The sidebar row says the same, such as "Loading model, 42%".
- Warming up shows once the model is in memory and the server gets ready to answer, so a long wait never looks stuck.
- A model MaxHQ runs stays loaded while agents use it. Once no agent used it for the time in Unload models after, 15 minutes to start with, it unloads to free its memory.

## The speed in the footer

<!-- id: local-models/speed -->

While a Local agent's turn runs, the composer's footer shows how fast the model reads your prompt and writes its answer, in tokens per second, and how much of its context is in use, such as "1,180 / 74 tok/s · 31k of 66k". Hover it for the same in words.

After the turn the numbers stay, dimmed, and the tooltip says Last turn.

An agent on a linked device that runs on this Mac's models shows the speed the Mac last measured for that model, since the Mac's server measures it.

## The Local models page

<!-- id: local-models/page -->
<!-- covers: settings/local-models -->

Open Settings and pick Local models. Device at the top picks whose models the page shows. A device whose maxd is too old for local models says so, and asks you to update it first.

- Hardware shows the device's memory and GPU. On a Mac it also says how much of that memory the GPU can use. MaxHQ runs models itself on a Mac with Apple silicon. A device with no GPU MaxHQ can use says Too slow for agents, with Add server URL to use a model running elsewhere, such as on a Mac.
- Default model is the model new Local agents on the device start on. Automatic picks the best model that's ready.
- Models lists the models a Mac can run with llama.cpp: their size, whether they fit, an estimated speed, and a score that names its source. Download, load, unload or delete them here. A download offers Pause, and a paused one Resume.
- Servers lists what Local agents run on: the one Run by MaxHQ, and each server you added. Add server URL adds one. See Add a server below.
- Linked devices can use this Mac's models: under the server Run by MaxHQ on a Mac. See Use this Mac's models from another device below.
- Local agent settings: Unload models after frees a model's memory once no agent used it for that many minutes, and 0 keeps it loaded. KV cache picks Full size (f16), the default, or Half size (q8_0), which takes half the memory for the context. Context size sets how much a Local agent can keep in mind. KV cache and Context size apply the next time a model loads.
- Fewer tools for local agents, on by default. See Fewer tools below.

The device keeps these settings with its other settings. A device that runs no models itself shows only Fewer tools for local agents under Local agent settings.

## Add a server

<!-- id: local-models/servers -->
<!-- covers: settings/local-models -->

A server such as Ollama, LM Studio, vLLM or llama.cpp that answers the Anthropic Messages API can run Local agents.

1. On the Local models page, click Add server URL under Servers. Your own server is picked at the top of the form.
2. Type a Name, such as Ollama on devbox, and its URL, such as `http://devbox.example.com:11434`.
3. Under Token, if the server asks for one, type its token, or leave it empty.
4. Click Test. The device asks the URL what answers there, and the form shows what kind of server it is, whether it works with Local agents and with how many tokens of context, and the models it offers. Test saves nothing.
5. Click Add to save the server, or Cancel to close the form. maxd keeps the token and never shows it again.

A server that doesn't answer the Anthropic Messages API can't run a Local agent, and Test says so. Each server's row has Test to check it again later, and Remove.

A cloud service that answers the Anthropic Messages API is added the same way, with its API key as the Token; see Cloud services below. For OpenRouter, pick OpenRouter at the top of the form instead; see Sign in with OpenRouter below.

A plain http URL to another machine says Not encrypted on the network, because anyone in between can read your prompts and code. Use https, such as `https://gpu-box.example.com`, an SSH tunnel or a VPN to keep it private.

## Cloud services

<!-- id: local-models/cloud -->
<!-- covers: settings/local-models -->

A cloud service is a hosted API you add as a server, not hardware you run. OpenRouter is one. Its list has hundreds of models, each under the name OpenRouter gives it, such as Z.ai: GLM 5.3 Flash. A server you added before this list was read whole shows only its first 20 models until you click Test on its row.

- Under Your own server, the Add server URL form has a Cloud service switch under Token. Test turns it on for a cloud service MaxHQ knows and off for anything else. Once you set it yourself, Test leaves it as you set it. OpenRouter's choice has no switch, since MaxHQ knows it's a cloud service. A cloud service's row under Servers says Cloud service.
- New agent lists each cloud service under Agent by its own name, such as OpenRouter, with a cloud icon. Local keeps the servers you run.
- A cloud agent goes by the service and its model, such as OpenRouter · Z.ai: GLM 5.3 Flash, in the sidebar and the agent's header, with a cloud icon. It never shows as Claude and never counts toward Claude's usage.
- To move an agent to a cloud service, open its Move menu and pick a model under Move to OpenRouter.
- A Local agent on a server you run writes at most 4,096 tokens in one answer. A cloud agent writes up to 32,000, or the model's own limit when that is lower.
- Fewer tools for local agents leaves cloud agents alone: they always get every tool.

A device whose maxd is too old for cloud services doesn't show the switch, and runs every server as Local.

## Sign in with OpenRouter

<!-- id: local-models/openrouter -->
<!-- covers: settings/local-models -->

OpenRouter runs many cloud models behind one API key. MaxHQ can get that key for you, so you don't copy it by hand, and adds OpenRouter as a cloud service.

1. On the Local models page, click Add server URL under Servers, then pick OpenRouter at the top of the form. Its URL, `https://openrouter.ai/api`, is set for you.
2. Under Spending limit, type an amount in dollars, such as 25, or leave it empty for no limit. OpenRouter stops the key once it has spent that much.
3. Click Sign in with OpenRouter. Your browser opens on OpenRouter, and the form says Finish signing in in your browser. Sign in there and allow MaxHQ. Cancel stops the sign-in.
4. The device gets a server named OpenRouter with a key of its own, and its models show in the model picker.

The device asks OpenRouter for the key and keeps it, and MaxHQ never shows it. On OpenRouter the key is named MaxHQ on, then the device's name, such as MaxHQ on devbox. To stop it, revoke it on OpenRouter's keys page. A device that has an OpenRouter server already gives that server the new key, and the form says so before you sign in.

If the sign-in fails, the form says why, and Try again starts a new one. To paste a key instead, click Get a key to open OpenRouter's keys page, make a key there, paste it under API key, and click Test and Add. A device whose maxd is too old to sign in asks you to update it, and takes a pasted key meanwhile.

The OpenRouter server's row has Sign in again. It runs the same sign-in with no spending limit and gives the server a new key. The server keeps its name and models, so agents on it keep working. The old key works until you revoke it on OpenRouter's keys page.

## Credits on OpenRouter

<!-- id: local-models/credits -->

For an OpenRouter server, MaxHQ shows what its key has spent and, for a key with a limit, what's left of it. maxd reads them from OpenRouter with the server's Token, so they're that key's figures.

- **Plan usage**, the usage popover, has a row for each such server below your accounts, under the server's name. Usage on the phone shows the same rows. A key with a limit shows a bar, such as "$7.42 left of $10.00", and when the limit starts over, such as Resets monthly. A key without a limit shows what it spent, such as "$2.58 spent" and "$0.31 today".
- The server's row on the Local models page shows the same in one line.
- The model picker adds the main figure to the server's name, such as "OpenRouter · $7.42 left".

maxd reads the figures when it starts, when you click Test, add the server or sign in with OpenRouter, after a turn on the server ends but at most once a minute, and every 15 minutes while an agent on it has run in the last hour. Hover the figures to see when they were read. When a read fails, the last figures stay, dimmed, and the tooltip says why. When OpenRouter asks it to slow down, maxd waits longer each time, up to an hour.

When the key reaches its limit, or the account runs out of credits, the agent's turn fails and says which, and what to do on OpenRouter.

A key's figures don't include your account's balance. To see it, add an admin key, which OpenRouter calls a management key:

1. On the Local models page, click Add admin key for your balance on the OpenRouter server's row.
2. Click Make one on OpenRouter to create a management key in your OpenRouter settings, and paste it under Admin key.
3. Click Add. maxd checks the key by reading the balance, keeps it and never shows it again.

The balance then leads, such as "$14.84 left of $20.00" of all the credits you bought, with what the server's key spent second, such as "This key: $2.58".

An admin key can create and delete keys on your OpenRouter account. MaxHQ keeps it on the device that has the server and uses it only to read your balance. To take it out, click Remove beside Admin key added on the server's row. Remove only takes it out of MaxHQ, so delete the key on OpenRouter too once you no longer need it.

## Use this Mac's models from another device

<!-- id: local-models/linked -->

A device linked to a Mac can run its Local agents on the models MaxHQ runs on that Mac. They go over the maxd link, so nothing listens on the network and you need no VPN.

- The switch is Linked devices can use this Mac's models, on the Mac's Local models page under Servers, below the server Run by MaxHQ. It's on by default.
- When you start a Local agent on the linked device, the model picker lists the Mac's models under This Mac, or under the Mac's name from another Mac's app.
- While the switch is off, the picker still lists the Mac's models, each turned off and saying Turned off on, then the Mac's name, such as Turned off on Mac. A row under them opens the Mac's Local models settings, where you can turn the switch on.

## Fewer tools

<!-- id: local-models/fewer-tools -->

Every tool an agent has is described in each request, and a small model has a small context to spare. Fewer tools for local agents, on by default, gives Local agents on servers you run less to read so they answer sooner.

- MaxHQ gives them only the tools they use most: sending files, opening a file, showing a visual, reading another agent and reading notes.
- Claude Code's Workflow, ScheduleWakeup and ReportFindings tools are turned off too.
- The dispatcher keeps all its tools when it runs on a local model.
- Agents on a cloud service always get every tool. See Cloud services above.

Turn it off on the Local models page, under Local agent settings, to give Local agents every tool.
