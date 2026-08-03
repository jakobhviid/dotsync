# dotsync roadmap

The single home for anything "for the future" — planned functionality, known gaps,
and deferred items. If it isn't here, it isn't planned. (No `TODO.md`, no scattered
`// TODO` comments — this file is the one place to look.)

## Planned

- **`dotsync undo` — design locked, ready to implement.** A per-run undo of the last
  destructive operation. Design decisions settled:
  - **Journal location: per-machine state dir**, matching the siblings exactly —
    `$DOTSYNC_STATE_DIR` (test override) → `$XDG_STATE_HOME/dotsync` →
    `~/.local/state/dotsync`. This is *state*, not config (XDG separates the two;
    an undo journal is machine-generated, not user-edited), and it is precisely
    what amdl (`~/.local/state/amdl/undo`, `$AMDL_UNDO_DIR`) and temper
    (`$XDG_STATE_HOME` → `~/.local/state/temper`, `$TEMPER_STATE_DIR`) do. The bytes
    an undo restores only ever existed on the machine that ran the op, so the
    journal is per-machine and does not travel; the *cross-machine* half of an op
    already rides the synced `dotsync.toml`, which needs no journal.
  - **No separate byte-store.** dotsync already preserves originals — the cloud copy
    (adopt / unadopt / group remove) or the `<path>.bak` (`install --adopt`). The
    journal is just a per-run `manifest.json` recording each entry's op + paths; undo
    reverses using those existing backups.
  - **Scope:** `adopt`, `install --adopt`, `unadopt`, `group remove` (the ops that
    move/replace real files). Plain `install`/`uninstall` toggle a symlink and are
    trivially reversible by re-running, so they're out. `relinked` (adopt-from-another-
    machine, content already matched) loses nothing, so it's out too.
  - **Reverses (with a structural after-guard; skip + report, never clobber):**
    `adopted` → rm our symlink, move cloud copy back to `$HOME`, drop the mapping
    (guard: target still our symlink → source). `backed-up-linked` → rm symlink,
    move `.bak` back (guard: symlink intact + `.bak` present). `restored` → rm the
    real file, re-symlink, re-add the recorded mapping (guard: target still equals
    the cloud copy — a file edited since is skipped). Any guard mismatch skips that
    item with a reason; undo never overwrites something changed since.
  - **Depth:** `dotsync undo` reverts the most recent run; `dotsync undo --list`
    shows recent runs; keep ~10 manifests, prune older.
  - **Note:** undoing an `adopt` moves the file back out of the cloud, so other
    machines lose it — the honest inverse of an adopt having shared it.
  - Build spec-first (write the SPEC section, then implement) with adversarial tests
    on each guard (D7/D8).
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
