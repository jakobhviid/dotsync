# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

Nothing major is queued — the core is complete (adopt / install / uninstall,
groups, doctor, undo, version-skew). Ideas that were weighed and set aside are
under "Considered and deferred."

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
