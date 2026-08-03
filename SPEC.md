# dotsync specification

The authoritative behaviour and schema contract. When code and this document
disagree, **the code wins and this doc is the bug** — fix the doc. Task-oriented
recipes live in [WORKFLOWS.md](WORKFLOWS.md); the invariants behind these rules are
in [PRINCIPLES.md](PRINCIPLES.md).

## Two files

dotsync reads and writes exactly two TOML files.

### `config.toml` — per-machine (not synced)

At `$XDG_CONFIG_HOME/dotsync/config.toml` (falling back to `~/.config/dotsync/`).
Records the only per-machine state:

| Key | Meaning |
|---|---|
| `sync_dir` | The dotsync folder inside the cloud provider. Stored `~`-collapsed. |
| `home` | The home base mapping targets resolve against (default `$HOME`). Stored `~`-collapsed. |

**Lenient read:** a missing file means "not configured yet" (`None`), never an
error; a malformed file surfaces a clear parse error. Written by `setup` and the
lazy first-run path. This file deliberately does **not** record which mappings are
linked — the symlinks on disk are the source of truth (PRINCIPLES.md).

### `dotsync.toml` — shared (lives inside the sync folder)

Mirrors `$HOME` inside the sync folder and syncs across machines like any other
file. Shape: an optional top-level `dotsync_version` stamp followed by an array of
`[[mapping]]` tables.

```toml
dotsync_version = "2.1.0"      # stamped on every save; see "Version skew" below

[[mapping]]
name = ".config/zed"           # required
```

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Path relative to the home base; also the item's path inside the sync folder (mirror layout). |
| `target` | no | Explicit `$HOME` target for all OSes (`~` expanded). Defaults to `<home>/<name>`. |
| `target_mac` / `target_linux` | no | Per-OS target override. Setting **only one** scopes the mapping to that OS (skipped on the other). |
| `mode` | no | Octal mode enforced on the sync copy (e.g. `"0600"`). Auto-set for known secrets. |
| `on_conflict` | no | `"fail"` (default) or `"adopt"`. |
| `group` | no | Group label; several mappings managed as a unit. |

**Lenient read, safe write:** unknown keys are ignored; duplicate `name`s (from a
hand-merged conflicted copy) are de-duplicated to the first seen. `save` sorts by
`name`, writes a documenting header, and stamps `dotsync_version`.

## Mapping state

`plan` classifies each mapping on this machine into exactly one state, from the
target's kind (symlink / real / absent) crossed with whether the sync copy is
present. The `code` is the token used in `--json`:

| `code` | Meaning | Fixable by |
|---|---|---|
| `skipped` | Doesn't apply on this OS | — |
| `linked` | Symlinked to the sync copy (active here) | — |
| `available` | In the cloud, not linked here | `install` |
| `local-only` | Real file here, not in the cloud | `adopt` |
| `healable` | Real file where the symlink should be, **content matches** the sync copy (atomic-save clobber) | `install` / `doctor --fix` |
| `diverged` | Real local file **and** sync copy exist and **differ** — a conflict | user, or `--adopt` |
| `dangling` | Symlinked correctly, but the sync copy isn't present (cloud not downloaded, or source deleted) | wait for sync, or `doctor --fix` clears an orphan |
| `foreign-symlink` | A symlink pointing somewhere other than the sync copy | user |
| `missing` | Neither a local target nor a sync copy exists | — |

A "linked" match also accepts a target whose symlink text differs but resolves to
the same cloud copy (a differently-normalized sync path).

## Conflict semantics

A conflict is only ever a `diverged` state — both sides real, content differing.
Identical content (bytes, or a whole matching directory tree) is `healable` and
relinked silently; it is never a conflict.

- `on_conflict = "fail"` (default): refuse; the user resolves by hand.
- `on_conflict = "adopt"`: the sync copy wins — back the local target up to
  `<target>.bak`, then link. `install --adopt` applies this for one run without
  editing the file.

Every destructive step backs up first; dotsync never overwrites in place
(PRINCIPLES.md).

## Secrets

- **Never git:** dotsync refuses a sync folder inside a git working tree and never
  invokes git.
- **Auto-mode:** paths matching known-secret patterns (`.ssh`, `.gnupg`, `.aws`,
  `*.pem`, …) are tagged with a restrictive `mode`. A whole adopted secret directory
  gets `0700` dirs / `0600` files enforced recursively. Modes are re-asserted on
  every `install` and `doctor --fix`, since cloud clients may rewrite files
  world-readable.

## Groups

Every mapping belongs to a group. Group names and mapping paths are a **disjoint
selector namespace**: a group name may not contain `/`, may not start with `.`, and
may not equal any mapping name (enforced both directions). So `install <name>`
resolves unambiguously to a group (all members) or one mapping.

## Discovery

With no configured folder, dotsync scans known cloud roots (Nextcloud, iCloud Drive,
Dropbox, OneDrive, Proton Drive, and generic `~/<Provider>/…` layouts) for a
`dotsync` folder. Interactively it offers the candidates plus "enter a path";
non-interactively it auto-picks a sole existing candidate or bails with guidance —
it never silently creates a folder.

## Version skew

`dotsync.toml` carries the version of the dotsync that last wrote it. On read, if
the file was written by a **newer** dotsync than the running build, dotsync prints a
one-line warning to stderr suggesting `brew upgrade dotsync`. It **never** prompts,
blocks, or auto-upgrades — the warning is safe under `--json` and on a non-TTY. An
absent or unparseable stamp is silent (lenient read).

## Undo

`dotsync undo` reverts the most recent **destructive** run. It is journaled
per-machine — the journal never travels, because the bytes an undo restores only
ever existed on the machine that ran the op (the cross-machine half of an op rides
the synced `dotsync.toml`).

- **Journal location:** `$DOTSYNC_STATE_DIR` → `$XDG_STATE_HOME/dotsync` →
  `~/.local/state/dotsync` (state, not config). One `<id>.json` manifest per run;
  the newest 10 are kept, older ones pruned.
- **No separate backup store:** dotsync already preserves originals — the cloud
  copy (`adopt` / `unadopt` / `group remove`) or the `<path>.bak`
  (`install --adopt`) — so a manifest only records each action's paths and reverses
  using those.
- **Scope:** `adopt`, `install --adopt`, `unadopt`, `group remove`. Plain `install`
  / `uninstall` just toggle a symlink and are reversed by the opposite verb, so they
  are not journaled.
- **Reverses, each with a structural guard that *skips* rather than clobbers:**
  - `adopt` → drop our symlink, move the cloud copy back to `$HOME`, drop the
    mapping. Guard: the target is still dotsync's symlink and the cloud copy exists.
    (Removes the file from the cloud, so other machines lose it — the honest inverse
    of the adopt.)
  - `install --adopt` → drop the symlink, move the `.bak` back. Guard: still our
    symlink and the `.bak` is present.
  - `unadopt` / `group remove` → drop the restored real file, re-symlink the cloud
    copy, re-add the mapping. Guard: the target is still the unchanged restored copy
    (`trees_equal` with the cloud copy). **If you edited it since, undo skips it and
    keeps your version.**
- **Guardrails:** undo re-adds/drops mappings that propagate to every machine, so
  it needs confirmation — an interactive prompt on a TTY, or `--yes` under `--json`
  / non-TTY (otherwise it bails). `--dry-run` previews. Per-item skips/failures do
  not change the exit status (a skip is reported, not fatal).
- `dotsync undo --list` shows recent runs (age, command, item count) and exits `0`.

## Exit codes and output

- **Non-zero is reserved for "the command could not run"** (not configured, bad
  path, unreadable/unwritable sync folder). A **per-item sweep failure never changes
  the exit status**: `install` / `uninstall` / `adopt` / `unadopt` / `group remove`
  exit `0` even with failures, surfacing them in the `⚠ N of M failed` headline and
  in `--json`. Gate automation on the per-item results or on `doctor`.
- `doctor` is a health **check**: it exits non-zero on an error-level problem (a real
  conflict, or a secret / the sync folder inside a git repo), `0` otherwise.
- **Stream split:** a command's result (status table, doctor report, `config`,
  `group list`, any `--json` document) → **stdout**; a mutating sweep's per-item log
  and all warnings/errors → **stderr**. `--json` writes one document to stdout and
  calls no human/colour helpers.

The exhaustive per-command `--json` shapes are documented in
[WORKFLOWS.md](WORKFLOWS.md#json-output---json).
