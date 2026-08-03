# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

- **Built-in merge (`dotsync merge <path>`).** The hard case of bringing a machine
  that already has its own config into a folder that already holds another machine's
  copy is today a manual / LLM-driven recipe (WORKFLOWS.md "Migrating a machine …").
  Promote it to a first-class verb: preview both sides, union additive text,
  deep-merge structured JSON, stop and ask on a genuine conflict, then hand the
  location to dotsync.

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
