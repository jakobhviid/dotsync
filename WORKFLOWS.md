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

Manage several mappings as a unit with a group label:

```sh
dotsync add ~/.claude/CLAUDE.md ~/.claude/settings.json --group claude
dotsync add ~/.claude/keybindings.json     # no --group on a terminal → group picker
dotsync install claude                     # group name is a selector
dotsync uninstall claude
```

Adopting multiple paths at once files them together. Without `--group` on a
terminal, adopt shows a picker of existing groups plus "New group…". In `status`
a group's members are shown indented under it; in the interactive picker a group
is one toggle-all row.

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
downloaded yet), foreign symlinks, secret-mode drift, and cloud "conflicted
copy" files. `--fix` repairs only the safe cases (relink when content matches;
chmod secrets back to their mode); genuine conflicts are left for you.

## Removing

```sh
dotsync uninstall .config/zed   # remove the symlink here; cloud copy stays
dotsync uninstall --all         # unlink everything on this machine
```

To stop syncing something everywhere, delete its `[[mapping]]` from
`dotsync.toml` and move the folder out of the cloud directory by hand.

## Invariants

- dotsync never writes into a git repository and refuses a sync folder inside
  one — secrets cannot leak to git through dotsync.
- Destructive steps always back up (`<target>.bak`) rather than overwrite.
- `install`/`doctor` are idempotent and self-healing; re-running is always safe.
