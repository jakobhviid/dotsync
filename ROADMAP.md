# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

- **Built-in merge (`dotsync merge <path>`).** The hard case of bringing a machine
  that already has its own config into a folder that already holds another machine's
  copy is today a manual / LLM-driven recipe (see WORKFLOWS.md "Migrating a machine
  … LLM-assisted merge"). Promote it to a first-class verb: preview both sides,
  union additive text, deep-merge structured JSON, stop and ask on a genuine
  conflict, then hand the location to dotsync.
- **`dotsync undo`.** A journaled, content-addressed undo of the last destructive
  operation (`adopt` / `unadopt` / `group remove` / `install --adopt`). dotsync backs
  originals up to `<path>.bak` today but has no single command that reverts a run —
  and only if the current on-disk state still hashes to what dotsync left, so a
  file the user changed afterwards is never clobbered.
- **Version-skew awareness.** Stamp the writing dotsync version into `dotsync.toml`;
  when a machine reads a file written by a *newer* build, surface a warning (never
  silently auto-upgrade, never prompt under `--json`/non-TTY). The sync folder is
  shared across machines that may run different dotsync versions — the one place a
  format change can bite.
- **Typed error layer.** A small `thiserror` enum with a serializable
  `{ title, detail, action }` so `dotsync … --json` errors carry a structured,
  agent-driveable recovery action (e.g. "back up and re-run with `--adopt`") instead
  of a bare message string.

## Known gaps / cleanups

- **Exit-code contract (decision pending).** Per-item sweep failures currently exit
  non-zero (documented in WORKFLOWS.md). The house convention is to exit `0` and make
  failure salient in the human headline (`⚠ N of M failed` — already in place),
  reserving non-zero for "the command could not run at all." Resolve which contract
  dotsync commits to, then align `exit_from` and the WORKFLOWS exit-code section.
- **Readable-names pass.** Some closures still use single-letter bindings
  (`|m|`, `|i|`, `|o|`) against the house CODE-STYLE ("name every binding"). Sweep
  them to real names as files are touched.
- **Clickable paths.** OSC 8 hyperlinks in `status` / the overview (link each mapping
  to its cloud copy or `file://` target), dropped cleanly on terminals that don't
  support them.

## Deferred / out of scope

- **Windows.** dotsync targets macOS and Linux only (the four release targets:
  `{x86_64,aarch64}-{apple-darwin,unknown-linux-musl}`). No Windows support planned.
