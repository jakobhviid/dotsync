# dotsync workflows

Task-oriented guide. Every command accepts `--json` for machine-readable output.

## Mental model

- A cloud-synced folder contains a `dotsync/` directory that **mirrors `$HOME`**.
  A mapping's `name` is its path relative to the home base (e.g. `.config/zed`)
  and is also its location inside that folder.
- `dotsync.toml` (inside the folder) is the shared list of mappings; it syncs
  across machines like everything else.
- Per-machine config (`~/.config/dotsync/config.toml`) records only two things:
  the sync folder location and the home base. **Which mappings are linked on a
  machine is determined by the symlinks on disk**, not by any stored list.

## First machine

No explicit setup needed — the first command that needs a sync folder discovers
it, prompts you to pick (or type a path), installs completions, and continues.

```sh
dotsync adopt ~/.config/zed  # first run prompts for the cloud folder, then adopts
dotsync adopt ~/.claude/CLAUDE.md
dotsync status               # confirm both show "linked"
```

To provision non-interactively (scripts, dotfiles bootstrap): `dotsync setup
~/Nextcloud/Apps/dotsync`. Inspect with `dotsync config`.

## Additional machine

```sh
dotsync            # first run finds the same cloud folder, opens the picker
```

The picker (a multiselect checklist, pre-checked to what's already linked here)
lets you tick exactly what you want on this machine. Non-interactively:
`dotsync install --all` or `dotsync install .config/zed`.

Or non-interactively:

```sh
dotsync install --all              # link everything applicable to this OS
dotsync install .config/zed        # link specific mappings by name
```

## Migrating a machine that already has its own config (LLM-assisted merge)

The hard case when bringing an existing desktop into the fold: it already has its
own `~/.claude/` memory, `settings.json`, shell rc files, etc., **and** the cloud
folder already holds versions from your other machines. A plain `install` or
`adopt` won't help here — `adopt` deliberately **refuses** when the cloud copy
already exists and differs ("resolve by hand, or delete one side"), precisely so
it never clobbers one machine's history with another's. The goal is to **merge**
both sides, write the result into the cloud copy (so every machine gets it), then
hand the location over to dotsync.

This is a one-time migration chore — once merged you never redo it. It's a good
fit for an AI agent driving dotsync; the steps below are written for one. Run it
per path that exists on **both** sides (local real file *and* cloud copy).

1. **Find the divergences.** `dotsync status --json` and `dotsync doctor --json`
   list mappings whose local `$HOME` file is a real file differing from the cloud
   copy (state `diverged`), plus local-only files not adopted yet. Also try
   `dotsync adopt <path>` — if it bails with "already exists in the cloud folder
   and differs", that path needs merging.

2. **Merge by file type, always writing the result into the cloud copy**
   (`<sync-dir>/dotsync/<name>`), never only into `$HOME`, so the merge
   propagates to every machine:
   - **Additive text** (`CLAUDE.md`, memory `*.md`, `.zshrc`, `.gitconfig`):
     union the content — keep every unique section/line from *both* sides, append
     this machine's local-only blocks to the cloud copy. Don't drop the other
     machines' entries.
   - **Structured JSON** (`settings.json`, `keybindings.json`): deep-merge
     objects and union arrays with de-duplication. Where the same key has a
     genuinely different value on each side, stop and ask the user which wins —
     never silently pick one.
   - **Anything opaque, or a genuine conflict you can't reconcile**: don't
     auto-merge; keep the backup (next step) and ask the user.

3. **Back up the local original first** (`cp <path> <path>.bak`) so every merge is
   reversible until the user has confirmed the result.

4. **Take over the location.** Once the cloud copy holds the merged result, make
   the local file **byte-identical** to it (`cp <sync-dir>/dotsync/<name> <path>`),
   then:
   - new path (not a mapping yet): `dotsync adopt <path>` — content now matches,
     so adopt removes the local file and symlinks it to the cloud copy
     (`relinked`) instead of refusing.
   - existing mapping: `dotsync install <name>` (or `dotsync doctor --fix`) — a
     real file matching the cloud copy is `healable`, so it's relinked cleanly.

   (Equivalent shortcut once the merge is in the cloud copy: set
   `on_conflict = "adopt"` on the mapping and `dotsync install <name>` — the
   now-merged cloud copy wins and the redundant local file is backed up to
   `.bak`.)

5. **Verify and clean up.** `dotsync status` should show every migrated path
   `linked` with no remaining conflicts, and `dotsync doctor` clean. Once the
   user confirms the merged config works, remove the `.bak` files.

The end state is identical to any other machine — real files replaced by symlinks
into the cloud folder — with this machine's local history folded into the shared
copy rather than lost.

## Adopting new config later

```sh
cd ~/.config && dotsync adopt zed          # name derived from the path under $HOME
dotsync adopt ~/.ssh/config                # secrets auto-tagged mode = 0600
dotsync adopt ~/.config/foo --linux        # linux-only mapping
```

`adopt` resolves the path against your current directory, computes its location
relative to the home base, moves it into the cloud folder, and symlinks it back.
Other machines pick it up as "available" and can link it via the picker or
`install`.

## Grouping related config

Every mapping belongs to a group, so related config is managed as a unit:

```sh
dotsync adopt ~/.claude/CLAUDE.md ~/.claude/settings.json --group claude
dotsync adopt ~/.claude/keybindings.json   # no --group on a terminal → group picker
dotsync install claude                     # group name is a selector
dotsync uninstall claude
```

Adopting multiple paths at once files them together. Without `--group` on a
terminal, adopt shows a picker of existing groups plus "New group…";
non-interactively it derives the name from the path (`~/.config/zed` → `zed`) and
echoes it in the outcome. Group names are a **disjoint namespace** from mapping
names — no `/`, no leading `.` — so a selector is never ambiguous. In `status` a
group's members are shown indented under it; in the interactive picker a group is
one toggle-all row.

Manage groups after the fact with the `group` verb:

```sh
dotsync group list                         # groups and their members
dotsync group rename claude claude-code    # relabel every member (merges if the name exists)
dotsync group move .config/zed editors     # reassign one mapping by its path name
dotsync group remove claude                # stop syncing the group everywhere (safe restore, below)
```

## Different paths per OS

When the same content lives at different paths on mac and linux, give both:

```toml
[[mapping]]
name = ".config/Code/User"
target_mac = "~/Library/Application Support/Code/User"
target_linux = "~/.config/Code/User"
```

Setting only one OS target scopes the mapping to that OS (skipped elsewhere).

## Keeping healthy

```sh
dotsync doctor          # report problems
dotsync doctor --fix    # relink atomic-save clobbers, re-assert secret modes
```

`doctor` flags: atomic-save clobbers (a real file where a symlink should be),
true conflicts (local differs from cloud), dangling links (cloud copy not
downloaded yet), foreign symlinks, secret-mode drift, cloud "conflicted copy"
files, and cross-machine drift — an orphan symlink pointing into the cloud folder
whose mapping was removed on another machine, or a group you use here that gained
members you haven't linked. `--fix` repairs only the safe cases (relink when
content matches; chmod secrets back to their mode); genuine conflicts and drift
are surfaced read-only for you to resolve (`adopt` an orphan to re-track it,
`install <group>` to link the newcomers).

## Removing

```sh
dotsync uninstall .config/zed          # remove the symlink here; cloud copy + mapping stay
dotsync uninstall --all                # unlink everything on this machine

dotsync group remove claude            # stop syncing a whole group everywhere
dotsync group remove claude --dry-run  # preview first; --yes skips the confirm
```

`uninstall` is per-machine: it drops the symlink but leaves the cloud copy and
the `[[mapping]]` in place, so other machines are untouched. To stop syncing a
whole group *everywhere*, `dotsync group remove <group>` restores its files to
`$HOME` on this machine (copy-the-cloud-copy → swap the symlink, so nothing is
lost and the cloud copy is kept), then removes the mappings from `dotsync.toml`
— which propagates to every machine. Because that edit is global it asks first
(or pass `--yes`) and supports `--dry-run`.

To stop syncing a *single* mapping everywhere, delete its `[[mapping]]` from
`dotsync.toml` and move the folder out of the cloud directory by hand (a
per-mapping `remove` isn't wired yet).

## Invariants

- dotsync never writes into a git repository and refuses a sync folder inside
  one — secrets cannot leak to git through dotsync.
- Destructive steps always back up (`<target>.bak`) rather than overwrite.
- `install`/`doctor` are idempotent and self-healing; re-running is always safe.
