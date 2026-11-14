<!--
SPDX-License-Identifier: Apache-2.0
Copyright (C) 2026 Max Boksem
-->

# Changelog

Each version of the maxhq.workflow standard. A minor version only adds,
and a major version may change anything (see
[Versions](v1/spec.md#versions)). Each release is tagged
`workflow-spec-<major>.<minor>` in this repository.

## 1.0

The first version.

- The descriptor, `maxhq.workflow/1.0`, with its
  [schema](v1/descriptor.schema.json).
- The run file's reference shape, with its [schema](v1/run.schema.json).
- The JSON Lines event log, with its [schema](v1/event.schema.json), and
  line logs read through a pattern.
- Discovery in the project, vendored folders and the user's library.
- Value sources: `path`, `count`, `sum`, `match` and `alive_pid`.
- Templates, predicates and matchers.
- Thirteen standard statuses, the standard rules and display rules.
- Groups, info, badges, links, detail lists and header lines.
- Attempt spans and loop counts from the event log.
- Binding agents by title, prompt, command, pid and the default
  `maxhq-node:` line.
- `runs.env` with a prefix per workflow.
- Overlays, `maxhq.workflow-overlay/1.0`.
- `x-` extension keys.

Changes to 1.0 before its release:

- A reader refuses a key the standard lacks, by its path, unless the
  descriptor is for a newer minor.
- A failed node fails the run only once the run's lead is idle or gone.
- A run needs you when its lead waits, as well as when a node does.
- Attempts pair with every agent bound to the node so far.
- Loops leave out targets the run omits and name the target they went
  back to.
- A `host` node binds its leads only once work on it has begun.
- A `runs.env` value can't replace or undo a variable the reader sets or
  removes for an agent, and a reader may refuse a name that could change an
  agent's login, what it runs or where its traffic goes.
- A reader sets `runs.env` variables on an agent only once the user allows
  them, having seen each name and value, and asks again when any changes.
  It marks a name that could change an agent's login, what it runs or
  where its traffic goes, rather than refusing the descriptor, and a value
  replaces or undoes what the reader set only with the user's consent.
