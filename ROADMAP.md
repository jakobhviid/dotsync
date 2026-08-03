# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

- **`dotsync merge <path…>` — design locked.** Assists the hard case of bringing a
  machine that already has its own config into a cloud folder that already holds
  another machine's copy (today a manual / LLM recipe — WORKFLOWS.md "Migrating a
  machine …"). The reframe that makes it safe: **dotsync does not become a general
  content merger.** Naive text/line union corrupts INI/`.gitconfig` and array union
  is ambiguous — a wrong merge silently loses config. dotsync owns only the provably
  safe and the fiddly-mechanical parts; the *judgment* stays with a human/agent.
  - **Precondition:** a path resolving to a `diverged` state — a real local file and
    a differing cloud copy (whether or not it's already a mapping; like `adopt`, a
    new one gets a mapping with `--group`). Non-diverged inputs get actionable
    redirects (`healable` → `install`; `local-only` → `adopt`; `linked` → nothing to
    merge).
  - **The only auto-merge — additive JSON.** If *both* sides parse as JSON and their
    deep object-merge has **no conflicting leaf** (each side only adds keys), take the
    union. A differing scalar **or array** at the same path is a conflict, never
    silently merged (arrays are opaque leaves — union semantics are ambiguous).
    Guarantee: result key-set ⊇ both inputs, no value changed or invented, nothing
    dropped.
  - **Safe take-over (reuse existing machinery).** On a safe merge: preview the keys
    it would add → confirm (or `--yes`) → back up local to `.bak`, write the merged
    JSON to the cloud copy, symlink local → cloud, upsert the mapping. This is
    `install --adopt`'s relink mechanic and records the same `Backup` undo action, so
    `dotsync undo` restores the local `.bak` (the additive cloud superset is kept —
    nothing lost).
  - **Everything else → delegate, don't guess.** For a non-JSON file or a JSON
    scalar/array conflict: report which leaf paths disagree, point at the two real
    files (local and `<sync>/<name>`), print the finish step (merge into the cloud
    copy, then `dotsync install <name> --adopt`), and **exit non-zero** (like
    `git merge` on conflict).
  - **`--json` (agent-drivable):** `{ path, state, mergeable, kind:
    "json-additive"|"conflict"|"opaque", added: [key paths], conflicts: [key paths],
    cloud_path, local_path, next }` — on a conflict the agent gets both paths + the
    conflict list, drives the merge, then calls `install --adopt`.
  - **Guardrails:** writes the shared cloud copy (propagates everywhere), so it
    confirms / needs `--yes` on non-TTY, and `--dry-run` previews without writing.
  - **Scope v1:** additive-JSON auto-merge + safe take-over + conflict surfacing.
    *Not* v1: text/line union, INI, or 3-way merges (unsafe to automate — delegated
    to the human/agent). Build spec-first with adversarial tests on the conflict
    detector (no silent value change; a differing array is a conflict, not a union).

## Known gaps / cleanups

- **Readable-names pass.** Some closures still use single-letter bindings
  (`|m|`, `|i|`, `|o|`) against the house CODE-STYLE ("name every binding"). Sweep
  them to real names as files are touched.
- **Clickable paths.** OSC 8 hyperlinks in `status` / the overview (link each mapping
  to its cloud copy or `file://` target), dropped cleanly on terminals that don't
  support them.

## Considered and deferred

- **Typed error layer (thiserror + serializable `UserFacing`).** The `tern` sibling
  uses this because its errors cross a D-Bus/GUI boundary and must serialize. dotsync
  has **no such boundary** — its only consumer is `--json`, whose error object is
  already useful — and the house CODE-STYLE says "use anyhow throughout; reserve
  typed errors for the rare case a caller must branch on the variant." A wholesale
  conversion would *fight* that guidance. Revisit only if a real branching consumer
  appears (e.g. a GUI/daemon front end); until then anyhow-with-context is correct.

## Deferred / out of scope

- **Windows.** dotsync targets macOS and Linux only (the four release targets:
  `{x86_64,aarch64}-{apple-darwin,unknown-linux-musl}`). No Windows support planned.
