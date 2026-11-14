<!--
SPDX-License-Identifier: Apache-2.0
Copyright (C) 2026 Max Boksem
-->

# The maxhq.workflow standard, version 1.0

This standard lets a workflow tell MaxHQ where its runs live and how to
read them, so MaxHQ can draw a run's nodes, statuses, history and agents
with no code of its own for that workflow. A workflow ships one file, the
descriptor. Its runs write plain files: a run file and, optionally, an
event log.

The fields of each file are defined by three JSON Schemas (draft 2020-12):

| File | Schema |
|---|---|
| Descriptor | [descriptor.schema.json](descriptor.schema.json) |
| Run file in the reference shape | [run.schema.json](run.schema.json) |
| One line of an event log | [event.schema.json](event.schema.json) |

This document covers what a schema can't say: where descriptors are found,
how values are read, the status rules, binding, history, versions and
security limits. It links to the schemas for fields rather than repeating
them. Where this document and a schema disagree, the schema wins.

The [examples](examples/) folder holds complete projects that use each
feature. [The changelog](../CHANGELOG.md) records each version.

## Conventions

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY" and
"OPTIONAL" in this document are to be read as described in BCP 14
([RFC 2119](https://www.rfc-editor.org/rfc/rfc2119) and
[RFC 8174](https://www.rfc-editor.org/rfc/rfc8174)) when, and only when,
they appear in all capitals, as shown here.

Terms used here:

- A **workflow** is a tool that runs work in steps, such as a build
  pipeline or an agent orchestrator.
- A **descriptor** is the file that describes one workflow to a reader.
- A **run** is one execution of a workflow. Its **run folder** holds its
  files, and its **run file** says where it stands.
- A **node** is one step of a run.
- A **writer** is the workflow, which writes the run's files.
- A **reader** follows runs through this standard. MaxHQ is one.
- The **project** is the folder a reader treats as a codebase's root.
- An **agent** is a coding agent session the reader knows about, such as
  a Claude or Codex session. A **lead** is an agent that started others,
  its **sub-agents**.

## Files

Descriptors and run files are YAML 1.2 or JSON, in UTF-8. A descriptor's
name ends in `.yaml`, `.yml` or `.json`. A reader MAY refuse any file
larger than 4 MiB, and MaxHQ does.

A YAML file MAY name its schema in a `yaml-language-server` modeline, so
editors offer completion and checks:

```yaml
# yaml-language-server: $schema=https://schemas.example.com/maxhq/workflow/v1/descriptor.schema.json
```

Each schema's `$id` gives its URL. The host in those URLs is a
placeholder until the schemas are published. A reader MUST NOT fetch a
schema to read a file.

## Where descriptors are found

A reader looks for descriptors in these places, in this order:

1. The project: `<project>/.maxhq/workflows/`.
2. Vendored workflows: `<project>/<dir>/.maxhq/workflows/` for each
   folder `<dir>` directly under the project, hidden ones included, in
   name order. `.git` and `node_modules` are skipped. A tool cloned into
   the project brings its descriptor this way.
3. The user's library, a folder of the reader's choosing.
4. Built in. MaxHQ has one, `maxhq.agents`, which draws a lead and its
   sub-agents as a run when no descriptor claims them.

Within a folder, files are read in name order. When two descriptors have
the same `id`, the first found wins and the others are passed over.
Overlays from any place apply to the winner (see [Overlays](#overlays)).
A file that doesn't load SHOULD be reported, and MUST NOT stop the others
from loading.

The **workflow folder** is the folder that holds a descriptor's
`.maxhq/`: the project for place 1, `<dir>` for place 2 and the library
folder for place 3.

A reader SHOULD look again from time to time, so new and changed
descriptors take effect without a restart.

### Finding runs

`runs.glob` matches each run's file, from the project's root. It MAY
start with `{project}/`. It MUST NOT be empty, start with `/` or contain a
`..` part. `*` and `?` stay inside one folder, `**` crosses folders and
`[...]` and `{a,b}` work as usual. A reader MAY limit how deep `**` goes
and how many entries it looks at. MaxHQ stops at 8 folders below the
glob's fixed start and 20,000 entries. `.git` and `node_modules` are
never entered.

The folder holding a run file is the run folder. Each run file belongs to
the first workflow, in the order above, whose glob matches it.

## The descriptor

The [descriptor schema](descriptor.schema.json) defines every key. In
short:

| Key | What it holds |
|---|---|
| `format` | `maxhq.workflow/<major>.<minor>`. REQUIRED |
| `id` | The workflow's id. REQUIRED |
| `name` | What a reader calls it. The id when left out |
| `revision` | The descriptor's own revision, a whole number |
| `runs` | Where runs live and what each says about itself. REQUIRED |
| `nodes` | How the run file lists its nodes, and per-node `info` |
| `groups` | A label and folded state per group |
| `rules` | Display rules |
| `badges` | Short labels on nodes |
| `loops` | Loops between nodes |
| `links` | Links a node offers |
| `bind` | Patterns that bind agents to nodes and runs |
| `detail` | Lists for a node's expanded row |
| `header` | Lines for the run's header |

An `id` is lower case letters, digits, `.`, `_` and `-`, and starts with a
letter or digit. Ids that start with `maxhq.` are reserved for MaxHQ and
MUST NOT be used by a workflow.

`revision` is the workflow's to bump. A reader only shows it.

Keys that start with `x-` are extensions. A reader MUST ignore them. A
descriptor MUST NOT have other keys the schema doesn't name. A reader
SHOULD refuse a descriptor with such a key when the descriptor is written
for the reader's minor version or an older one, and name the key's path.
A reader SHOULD ignore keys it doesn't know in a descriptor written for a
newer minor version (see [Versions](#versions)).

## Values

Many keys take a **value**: a literal, or a **source** that reads one
from a file. A source is a map with exactly one of these keys:

| Key | Gives |
|---|---|
| `path` | The value a [JSONPath](https://www.rfc-editor.org/rfc/rfc9535) selects. Several matches give a list. Nulls are dropped |
| `count` | How many values a JSONPath selects. A path that selects one list counts its items |
| `sum` | The sum of the numbers a JSONPath selects. `each` changes each number first: `add` is added, then `min` and `max` clamp it |
| `match` | A regular expression, as a template, over the lines of a file. `pick` is `first` (the default), `last`, `count` or `all`. `group` picks a group by number or name, the whole match when left out |
| `alive_pid` | `true` while the process whose pid the named file holds is running, else `false` |

A source MAY also have:

- `in`: the file to read, as a template. It is relative to the run folder,
  or starts with `{project}` or `{workflow}`. The run file when left out.
  `path`, `count` and `sum` parse it as YAML or JSON. `match` reads its
  lines.
- `default`: the value when the source reads nothing.

`each` goes only with `sum`, and `pick` and `group` only with `match`.
In a `match` pattern, values filled into the template are escaped, so they
match as plain text.

A source reads nothing when its file is missing, too large or doesn't
parse, when its path selects nothing, or when its `in` path would leave
both the project and the workflow folder. A literal map that isn't a valid
source is an error in a descriptor for this version or older.

Regular expressions everywhere in this standard use the syntax of Rust's
[regex](https://docs.rs/regex/latest/regex/#syntax) crate. Named groups
are written `(?<name>...)`. There are no look-arounds or back-references.

### Templates

Text that says it is a template fills `{name}` placeholders. A name
starts with a letter or `_` and holds letters, digits, `_`, `.` and `-`.
Braces around anything else stay as they are, so globs and counts such as
`\d{3}` survive.

| Name | Fills in |
|---|---|
| `{id}`, `{status}`, `{group}`, `{attempts}`, `{label}`, `{agent}` | The node's field. `{status}` is the workflow's own word |
| `{<field>}` | Any field the descriptor lists in `nodes.fields` |
| `{run.id}`, `{run.label}`, `{run.<fact>}` | The run's id, label and named facts |
| `{vars.<name>}` | A label's `vars`, then the node's `info.vars` |
| `{value}` | The value of the rule, badge or link |
| `{project}`, `{workflow}` | The project and the workflow folder |

Strings, numbers and flags fill in as text. A list fills in as its items
joined by `, `. When any name in a template has no value, or its value is
a map or null, the whole template reads nothing. A badge, header line,
link or note whose template reads nothing isn't shown.

### Predicates

A **predicate** tests a node and its run. Every key given MUST hold:

| Key | Holds when |
|---|---|
| `status` | The node's standard status is this one, or one of this list |
| `node` (or `id`) | The node's id is this one, or one of this list |
| `field` | Each named node field passes its matcher |
| `fact` | Each named run fact passes its matcher. Or, as one fact's name with `exists`, the run has or lacks that fact |
| `value` | The value of the rule, badge or link passes the matcher |
| `any_node` | Some node of the run passes this predicate |
| `orchestrator` | The run's lead stands as this, or one of this list: `busy`, `waiting`, `idle` or `gone` |
| `all`, `any`, `not` | Every one, at least one, or not this one holds |

A **matcher** tests one value. It is one of:

- A plain value: equal to it.
- A list: equal to one of its items.
- A map of comparisons, all of which MUST hold: `exists` (a flag), `eq`,
  `ne`, `gt`, `gte`, `lt` and `lte`. The other side of a comparison is a
  plain value or `{fact: <name>}`.

Equality reads `1` and `1.0` as the same number, and a string as equal to
a number or flag with the same text. `gt`, `gte`, `lt` and `lte` compare
numbers, and strings that read as numbers. A missing value passes only
`exists: false` and `ne`.

## The run-state contract

### The run file

The run file is any YAML or JSON file that `runs.glob` matches. A reader
reads it whole whenever it changes. A writer SHOULD replace it atomically,
for example by writing a new file and renaming it over the old one. When
it doesn't parse, a reader SHOULD try again a few times and then keep the
last state that did, with a notice. MaxHQ tries 3 more times, 20 ms apart.

The **reference shape**, defined by the [run schema](run.schema.json),
needs no mapping in the descriptor:

```yaml
run: { id, status, label, agent }
nodes:
  - { id, status, needs: [], group, attempts, label, agent }
```

A run file in another shape maps its own keys:

- `runs.id`, `runs.label`, `runs.agent` and `runs.status` are values.
  Left out, they read `$.run.id`, `$.run.label`, `$.run.agent` and
  `$.run.status`. A run with no id takes its run folder's name. A run with
  no label takes its id.
- `runs.facts` names more values about the run, for templates and
  predicates.
- `nodes.list` is a JSONPath to the node list, `$.nodes` when left out.
  When it selects one list, its items are the nodes. Otherwise each
  selected value is a node.
- `nodes.id`, `needs`, `group`, `status`, `attempts`, `label` and `agent`
  name each node's keys. Each defaults to its own name.
- `nodes.fields` lists more node keys that templates and predicates may
  read.

A node is a map. One with no id, or with an id seen before in the same
run, is passed over. `needs` is a list of node ids or a single id. The
nodes are drawn in the order the run file lists them. A `needs` id that
isn't in the run draws no edge.

A run's id MUST be unique in its project, and a node's id unique in its
run.

### The event log

A run MAY keep an event log in its run folder. It gives a reader the
run's timeline, attempt spans and loop counts. `runs.events` lists the
logs to look for, in order of preference. A reader reads the first that
exists. A log's path MUST stay inside the run folder.

`{jsonl: <file>}` is a [JSON Lines](https://jsonlines.org/) file: UTF-8,
one JSON object per line, each line ending in `\n`. A `\r` before the
`\n` is ignored. The [event schema](event.schema.json) defines one line:

```json
{"t":"2026-11-01T14:02:07.120Z","node":"build","status":"busy","attempt":1}
```

`t` and `node` are REQUIRED, plus `status` (the workflow's own word,
mapped like a node's status), `note`, or both. `attempt` and `agent` have
meanings below. Other keys MAY be added and a reader MUST keep or ignore
them, never fail on them.

`{lines: <file>, match: <pattern>}` reads any line log. On each line that
matches, the named groups `t`, `node` and `status` give the event, and
`note` MAY add a note. A line with neither `status` nor `note` isn't an
event.

For both kinds:

- A writer MUST only append. A reader reads from where it stopped. A last
  line with no `\n` yet is still being written, and a reader waits for
  it. A log that got shorter was replaced, and a reader starts it over.
- A line that doesn't parse is passed over.
- `t` is an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339) time. One
  with no offset is read as UTC. When a time is earlier than the event
  before it, a reader uses the earlier event's time, so the timeline never
  runs backwards.
- A reader MAY keep only the newest events. MaxHQ keeps 20,000 per run.

With no log, a reader SHOULD keep its own history of the status changes it
sees in the run file while it watches.

### Agent ids

MaxHQ sets `MAXHQ_AGENT=<agent id>` on agents it starts. A writer that
copies it into the run's `agent` binds the run to its lead at once. A
node's `agent` and an event's `agent` bind that agent to the node the same
way. Agent ids are UUIDs. A value that isn't one binds nothing.

## Statuses

Every node has one of the standard statuses:

| Status | Means |
|---|---|
| `omitted` | The descriptor names the node, but this run leaves it out |
| `queued` | It waits for the nodes it needs |
| `ready` | Everything it needs is done, and it can start |
| `busy` | It's running |
| `waiting_child` | It waits for a child run or agent |
| `waiting_human` | It waits for a person |
| `done` | It finished |
| `skipped` | It was skipped |
| `will_skip` | It will be skipped when the run reaches it |
| `failed` | It failed |
| `blocked` | It can't run, for example because something it needs failed |
| `stale` | It says it's busy, but nothing is working on it |
| `unknown` | The workflow wrote a word the standard doesn't know |

A reader works out each node's status in these steps:

1. **Mapping.** The workflow's word goes through `nodes.status_map`. A
   word missing from the map that is a standard status reads as itself.
   Any other word reads as `unknown`. A node with no status reads as
   `queued`. A node that `nodes.info` names but the run doesn't list is
   `omitted`.
2. **The standard rules.** These are the same for every workflow, and
   each reads the statuses as step 1 left them:
   - `queued` becomes `ready` when every node it needs is `done`,
     `skipped` or `omitted`, or isn't in the run.
   - `busy` becomes `stale` when the run's lead is `idle` or `gone` and no
     agent bound to the node is alive. Only a reader that knows the
     project's agents applies this rule.
3. **The descriptor's rules.** For each node, the first rule in `rules`
   whose `when` holds wins. Its `is` replaces the status, its `note` (a
   template) becomes the node's note, and its `provisional`, `true` or a
   predicate, marks the status as a guess. A rule with no `is` keeps the
   status and still sets the note. A rule's `value` is what its `value`
   predicate tests. Rules see the statuses as step 2 left them.

A node with no note from a rule shows the last `note` its events gave.

A node is **quiet** when every live agent bound to it has written nothing
for 10 minutes and none has a tool call open. A reader SHOULD show it.

The run's own status is `runs.status`, mapped like a node's. When the run
file gives none, it comes from the nodes: `done` when every node that
isn't `omitted` is `done`, `skipped` or `will_skip`, else `failed` when
any node failed and the run's lead is `idle` or `gone`, else
`waiting_human`, else `busy`, else `queued`, the first that any node has.
While the lead is `busy` or `waiting` it may still retry a failed node,
so a failed node then counts as `busy`. The run's progress counts `done`
and `skipped` nodes against every node that isn't `omitted`.

A run **needs you** when a node is `waiting_human`, or when the run's lead
is `waiting` and the run isn't `done`, `failed` or `skipped`. A lead that
waits on a question, such as whether to retry past a spent limit, needs
you even when no node says so.

## Display

- `nodes.info` gives, per node id, a `label`, a `lane` other than the
  main one (such as `side`), `host` (the lead works on the node itself,
  so the lead binds to it) and `vars` for templates. A node's label is
  `info.label`, else its `label` field, else its id.
- `groups` gives, per group id, a `label` and whether it starts
  `collapsed`. Groups are drawn in the order their nodes first appear.
- `badges` puts short labels on nodes. `node` limits a badge to some
  nodes. `text` is a template, `tone` is `neutral` (the default), `info`,
  `good`, `warn` or `bad`, and `when` says when it shows. `value` and
  `vars` give values for the text.
- `links` offers links on nodes. The `url` is a template, and only URLs
  to `http://127.0.0.1` or `http://localhost` are allowed, both in the
  descriptor and once filled in.
- `detail` gives lists for a node's expanded row. `list` is a value whose
  items are the list. `limit` keeps only the last items.
- `header` gives lines for the run's header, like badges with no node.

## Attempts and loops

A node's **attempts** come from its history. A `busy`, `waiting_child` or
`waiting_human` event opens an attempt. Another such event continues the
open attempt when either has no `attempt` number or both have the same
one. A new number starts a new attempt. Any other status closes the open
attempt and records how it ended. Agents bound to the node pair with
attempts by start time, unless an event names its agent. That counts
every agent bound to the node so far, so an attempt keeps its agent after
the agent leaves.

`loops` describes work that goes back. Each loop has a `from` node and
`to`, one or more node or group ids. Its count is how many times a `to`
node, or a member of a `to` group, went `busy` again after `from` went
`busy`. Going `busy` means an event that makes a node `busy` when it
wasn't. `budget`, when given, is how many rounds the workflow allows. A
reader shows it and doesn't enforce it.

A reader leaves out the `to` ids that name no node of the run, where a
node that is `omitted` isn't of the run, and a group counts while it has
a member of the run. It leaves out a loop whose `from` isn't of the run
or that has no `to` left. A loop shows going back to the `to` it last
went back to, or to its first `to` before it has.

## Binding agents

A reader binds agents to runs and nodes from facts it already keeps about
each agent. A reader MUST NOT need to read an agent's transcript to bind
it. Patterns in `bind` use named groups `run` and `node`. A pattern with
a `run` group binds only when the group equals the run's id. One with no
`node` group binds no node.

| Pattern | Matched against |
|---|---|
| `bind.node[].title` | The agent's title. For a Claude sub-agent, its task's description |
| `bind.node[].prompt` | The first 4 KB of the agent's first prompt. `and` adds a second pattern the same text MUST also match, and its groups count too |
| `bind.run[].command` | The text of each finished command the agent ran. A match binds the agent as the run's lead. A reader MAY check only commands that contain the pattern's literal start |
| `bind.process[].pid` | A value, read per node, that gives a pid. The agent with that pid binds to the node |

The run's `agent`, a node's `agent` field and an event's `agent` bind
too.

When a descriptor has no `bind` key, the default applies: a line in the
prompt of the form

```text
maxhq-node: <run id> <node id>
```

binds the agent to that node. The line matches
`(?m)^\s*maxhq-node:\s*(?<run>\S+)\s+(?<node>\S+)\s*$`. A writer can put
it in the prompt it dispatches each node with and need no `bind` block. A
descriptor with a `bind` key and no `node` patterns turns the default off.

The run's **leads** are its `agent`, then the leads of its bound
sub-agents, then agents a `command` pattern bound. The lead's state for
the `orchestrator` predicate is `busy` when any lead is busy, else
`waiting`, else `idle`, else `gone`. A node marked `host` binds every lead
too, once work on it has begun: not while it is `omitted`, `queued`,
`ready`, `will_skip`, `skipped` or `blocked`.

Binding is many to many. A run can span several agents, for example after
a resume, and an agent can work on several nodes. Agents on one node are
listed oldest first.

## Environment

`runs.env` names variables a reader sets on agents it starts in a
project the descriptor applies to. Each name MUST be upper case letters,
digits and `_`, and MUST start with the workflow's id in upper case, with
each character that isn't a letter or digit turned into `_`, then `_`.
For the id `pipeline` that's `PIPELINE_`. A reader SHOULD let the user
turn this off per project.

A reader MUST NOT set a `runs.env` variable on an agent until the user
has allowed it, having seen each name and value. A reader SHOULD hide a
value that looks like a credential until the user asks to see it, and
SHOULD ask again when a name or value changes, saying what changed. A
reader MAY keep the user's answer for the project.

A reader SHOULD mark a name that could change an agent's login, what it
runs or where its traffic goes, such as `NODE_OPTIONS` or `HTTP_PROXY`,
and say why. Ids such as `git` and `cargo-home` spell those names; a
descriptor that names one still loads, and the user decides. A reader
MUST NOT let a `runs.env` value replace or undo a variable it sets or
removes for that agent unless the user allowed it after being told it
does.

## Versions

`format` is `maxhq.workflow/<major>.<minor>`. A minor version only adds.
A new major version MAY change or remove anything.

- A reader MUST load a descriptor with its own major and an older or equal
  minor.
- A reader SHOULD load a descriptor with its own major and a newer minor,
  ignore what it doesn't know, and say the descriptor was made for a newer
  reader.
- A reader MUST NOT load a descriptor with a newer major. It SHOULD draw
  those runs another way, and say a newer reader is needed. MaxHQ draws
  them with `maxhq.agents`.
- From version 2.0 on, a reader converts a descriptor with an older major
  on load, one major at a time.

The run file and event log carry no version. Their reference shapes
change only with the descriptor's major.

### Overlays

An **overlay** changes one workflow's descriptor without editing it. It
lives in any of the places descriptors do, and has:

```yaml
format: maxhq.workflow-overlay/1.0
workflow: <the id it changes>
```

The rest merges over the descriptor key by key, as a JSON merge patch
([RFC 7396](https://www.rfc-editor.org/rfc/rfc7396)): a map merges into
the map under it, any other value replaces it, and `null` removes the key.
So a list replaces the list under it. `format` and `workflow` are never
merged. Overlays apply in the order they were found, and the merged
descriptor MUST be valid. An overlay follows the same version rules as a
descriptor.

## Security

A workflow's files are input a reader doesn't trust. A reader following
this standard:

- MUST NOT run any of a workflow's scripts or commands.
- MUST NOT write into a run folder or a workflow folder.
- MUST NOT read outside the project and the workflow folder. It checks
  `runs.glob`, every `in` path and every pid file after filling templates
  and removing `.` and `..` parts. It SHOULD NOT follow a symbolic link
  out of those folders.
- MUST read event logs only inside their run folder.
- MAY refuse files over a size limit, and SHOULD limit how much of a log it
  reads at once.
- MAY limit how deep a predicate nests, how many nodes it reads from a run and
  how many tests one pass over a run makes, and SHOULD say so on the run when
  it stops early. MaxHQ refuses a predicate more than 16 levels deep and reads
  at most 2000 nodes.
- MUST NOT open a link that doesn't go to `127.0.0.1` or `localhost`.
- MUST NOT set a variable outside the descriptor's `runs.env` prefix, or
  before the user allows it.
- MUST NOT fetch anything named in a descriptor, its schema included.

A reader keeps the first 4 KB of an agent's first prompt only to match
`prompt` patterns. MaxHQ keeps it in its daemon's memory and never sends
it to the app.

## Examples

Each folder in [examples](examples/) is a whole project:

- [reference](examples/reference/) writes the reference shape and maps
  nothing. It shows a JSON Lines event log, a group, a count fact, a badge
  on attempts, a loop and the default `maxhq-node:` binding.
- [mapped](examples/mapped/) maps a JSON run file of its own. It shows
  every kind of source, `status_map`, `info` with an omitted node and a
  side lane, each kind of rule, badges with values, a local link, every
  bind pattern, a line log, `runs.env` and an `x-` key.
- [vendored](examples/vendored/) ships its descriptor in a vendored
  folder, reads a file through `{workflow}`, keeps no event log and has an
  overlay in the project.
