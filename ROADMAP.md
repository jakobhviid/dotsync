# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

Nothing major is queued — the core is complete (adopt / install / uninstall,
groups, doctor, undo, version-skew). Near-term work is the cleanups below; larger
ideas that were weighed and set aside are under "Considered and deferred."

## Known gaps / cleanups

- **Readable-names pass.** Eliminate every single-letter / cryptic binding — locals,
  loop variables, and closure arguments alike (`r` → `row`, not just closures) — per
  the house CODE-STYLE ("no single-letter or cryptic names, anywhere").

## Considered and deferred

- **`dotsync merge` verb.** Once the headline feature, then set aside: now that
  `install --adopt` backs the local side up to `.bak` and `dotsync undo` makes that
  take-over reversible, the scary part of a cross-machine migration (clobbering a
  machine's config) is already defused. `status` / `doctor` surface the `diverged`
  state, and `install --adopt` + `undo` are the safe, reversible take-over. The
  *only* thing a merge verb would uniquely add is additive-JSON auto-union — a narrow
  convenience an agent does trivially (deep-merge the two files, then
  `install --adopt`) — which doesn't justify a JSON conflict-detector + command
  surface. The migration stays the documented WORKFLOWS recipe. Revisit only if
  auto-union proves a common, painful need. (The full merge design, if ever wanted,
  is in this file's git history.)
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
