# dotsync

Sync your user-level config between machines through a **cloud folder** — not git.

dotsync symlinks paths from your `$HOME` into a folder inside Nextcloud (or
OneDrive, Dropbox, Proton Drive, iCloud, …). The cloud provider already syncs
that folder across your machines continuously and with no merge ceremony, so
your editor settings, `~/.claude/` memory, API keys and dotfiles just stay in
sync. dotsync's job is only to wire the symlinks and stay out of the way.

Why not a git-based dotfile manager? Because for everyday config tweaks you
don't want to commit, pull, and resolve merges — you want the change to
propagate the way a synced folder already does. If you run Nextcloud/OneDrive
anyway, dotsync turns it into a dotfile sync with zero extra services.

```sh
brew install jakobhviid/tap/dotsync
# or:
curl -fsSL https://raw.githubusercontent.com/jakobhviid/dotsync/main/install.sh | sh
```

## How it works

- Your cloud folder holds a `dotsync/` directory that **mirrors your `$HOME`
  tree**: `~/.config/zed` lives at `dotsync/.config/zed`, `~/.claude/CLAUDE.md`
  at `dotsync/.claude/CLAUDE.md`. The mirror layout means the "original
  location" of everything is self-evident and names never collide.
- A `dotsync.toml` in that folder lists the mappings. It syncs with everything
  else, so every machine sees the same list.
- On each machine, dotsync symlinks the applicable items back into `$HOME`.
  **Which items are active on a machine is recorded by the symlinks themselves**
  — there is no per-machine state file to drift.

## Quick start

There is no mandatory setup step. The first time you run dotsync with no config,
it auto-discovers your cloud folder, lets you pick it (or type a path), installs
shell completions, and continues:

```sh
dotsync                      # first run: detect cloud folder → pick → picker opens
dotsync adopt ~/.config/zed  # move zed's config into the cloud, link it back
dotsync doctor               # health check (add --fix to repair)
```

On a second machine, the same first run finds the shared cloud folder and lets
you tick the items you want there. Run `dotsync setup` explicitly only to
re-point the folder or reinstall completions; `dotsync config` shows what's
configured.

## Commands

| Command | What it does |
|---|---|
| `dotsync setup [dir]` | Provision this machine and install completions. Usually unnecessary — the first run of any command offers this automatically. With no `dir`, auto-discovers a `dotsync` folder across common cloud providers (or type your own path). |
| `dotsync config` | Show the resolved sync folder, home base, and config path. |
| `dotsync` / `dotsync status` | Show the overview: every mapping and its state on this machine. Bare `dotsync` opens the interactive picker on a terminal. |
| `dotsync adopt <paths…> [--group <name>] [--mac\|--linux]` | Move existing `$HOME` files/dirs into the cloud folder and symlink them back. Chain several paths to adopt them together. Every mapping belongs to a group: `--group` sets it, otherwise it's derived from the path (interactive picker on a terminal, auto-derived and echoed non-interactively). `--mac`/`--linux` scopes to one OS. |
| `dotsync install [names…] [--all] [--dry-run]` | Link mappings on this machine. No args on a terminal opens the picker; `--all` links everything applicable. A name matches a group (all its members) or a single mapping. |
| `dotsync uninstall [names…] [--all] [--dry-run]` | Remove dotsync's symlinks here (the cloud copies stay). |
| `dotsync group <list\|rename\|move\|remove>` | Manage groups. `list` shows groups and their members; `rename <old> <new>` relabels a group (merges if `<new>` exists); `move <path> <group>` reassigns one mapping; `remove <group>` restores its files to `$HOME` (cloud copies kept) and drops the mappings from `dotsync.toml` on **every** machine — asks first (or `--yes`), supports `--dry-run`. |
| `dotsync doctor [--fix]` | Detect atomic-save clobbers, conflicts, dangling/foreign links, secret-mode drift, cloud conflict copies, and cross-machine drift (orphan symlinks whose mapping was removed elsewhere, partially-linked groups); `--fix` repairs the safe ones, drift is surfaced read-only. |

Global flags: `--json` (machine-readable output on every command) and `--llm`
(print the full guide for an AI agent).

## `dotsync.toml`

```toml
[[mapping]]
name = ".config/zed"          # path relative to home; also its path in the cloud folder

[[mapping]]
name = ".claude/CLAUDE.md"

[[mapping]]
name = ".config/Code/User"    # same content, different path per OS
target_mac = "~/Library/Application Support/Code/User"
target_linux = "~/.config/Code/User"

[[mapping]]
name = ".ssh/config"
mode = "0600"                 # enforced on the cloud copy; auto-set for known secrets

[[mapping]]
name = ".config/linearmouse"
target_mac = "~/.config/linearmouse"   # mac only (no linux target) → skipped on linux
```

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Path relative to the home base. Also its location inside the cloud folder. |
| `target` | no | Explicit `$HOME` path for all OSes. Defaults to `~/<name>`. |
| `target_mac` / `target_linux` | no | Per-OS path override. Setting only one scopes the mapping to that OS. |
| `mode` | no | Octal mode enforced on the cloud copy (e.g. `0600`). Auto-set for `.ssh`, `.gnupg`, `.aws`, `*.pem`, … |
| `on_conflict` | no | `fail` (default) or `adopt` (cloud wins, local backed up to `.bak`). |

## Groups

Every mapping belongs to a **group**, so related config is managed as a unit.
Chain paths when adopting to file them together; pass `--group`, or let dotsync
derive the name from the path (`~/.config/zed` → `zed`) — on a terminal it offers
a picker, non-interactively it uses the derived name and echoes it. Group names
live in their **own namespace**: they can't contain `/` or start with `.` (that's
the mapping-path namespace), and dotsync additionally forbids a group and a
mapping from sharing a name — so a selector like `dotsync install <name>` is
never ambiguous between the two.

```sh
dotsync adopt ~/.claude/CLAUDE.md ~/.claude/settings.json --group claude
dotsync adopt ~/.config/zed                       # derives and assigns group "zed"
dotsync install claude                            # a group name works as a selector

dotsync group list                                # groups and their members
dotsync group rename claude claude-code           # relabel (merges if the name exists)
dotsync group move .config/zed editors            # reassign one mapping
dotsync group remove claude                       # restore its files here, un-sync everywhere
```

In `status` a group shows its members indented under a `group · N/M linked`
header; in the interactive picker a group collapses to a single row. `group
remove` is the safe way to stop syncing a whole group everywhere — it restores
the real files here (cloud copies kept) before dropping the mappings, and warns
that the removal propagates to every machine. Everything else (mirror layout,
per-file state, self-healing) is unchanged.

## Self-healing

Many apps save atomically — they write a temp file and rename it over your
config, which silently **replaces the symlink with a real file** and stops the
syncing. dotsync notices: if the new file's content still matches the cloud
copy, `install`/`doctor --fix` just relinks it. If it genuinely diverged, that's
a conflict you resolve (or set `on_conflict = "adopt"`). Prefer mapping whole
directories where you can — an atomic save *inside* a symlinked dir replaces a
file within it, not the dir link, so it can't break.

## Secrets

Putting API keys and `.ssh/config` in a trusted cloud is a deliberate, supported
choice — but two things matter:

- **Nothing ever reaches git.** dotsync only moves files into your cloud folder;
  it never commits anything, and it refuses to use a sync folder that lives
  inside a git repository.
- **Modes are enforced.** Cloud clients often write files `0644`; dotsync tags
  known-secret paths with a restrictive `mode` and re-asserts it on `install` and
  `doctor --fix`. Whole secret directories are safe to adopt: `dotsync adopt
  ~/.aws` tags the directory `0700` and enforces `0700` dirs / `0600` files
  recursively within it (re-asserted on `install` and `doctor --fix`).

## AI disclosure

Parts of this codebase were written with the assistance of AI coding agents. All
changes were reviewed by the maintainer.

## License

MIT.
