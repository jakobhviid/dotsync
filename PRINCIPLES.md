# dotsync principles

The design invariants dotsync upholds. When a change is in tension with one of
these, the principle wins — or the principle changes, deliberately, here.

## The filesystem is the source of truth

Which mappings are linked *on a machine* is recorded by the symlinks on disk, not
by any stored list. There is no per-machine "what's active" file to drift out of
sync with reality. `plan` reads the world (`$HOME` + the sync folder) and classifies
each mapping's [`State`] from what it actually finds; every other command acts on
that classification. Per-machine config holds only two facts — where the sync folder
is and what the home base is — because everything else is discoverable.

## Never destroy data without a backup

Every destructive step is reversible. Replacing a real file with a symlink, or
letting the cloud copy win a conflict, first copies the original to `<path>.bak`.
Restoring a mapping copies the cloud copy back to `$HOME` before removing the link,
so nothing is lost if the cloud copy later disappears. dotsync never overwrites in
place.

## Secrets never reach git, by construction

Putting API keys and `.ssh/config` in a trusted cloud is supported — leaking them
into a git history is not. dotsync **refuses** a sync folder that lives inside a git
working tree, and it never runs git itself. Known-secret paths (`.ssh`, `.gnupg`,
`.aws`, `*.pem`, …) are tagged with a restrictive `mode` that is re-asserted on every
`install` and `doctor --fix`, because cloud clients often rewrite files world-readable.

## Idempotent and self-healing

Re-running `install` or `doctor --fix` is always safe. An atomic-save that clobbered
a symlink with an identical-content real file is silently relinked; a secret whose
mode drifted is re-chmod'd. An operation that has already happened is a no-op, never
a second mutation.

## Mirror the home tree

The sync folder mirrors `$HOME`: a mapping's `name` is its path relative to the home
base *and* its path inside the folder (`~/.config/zed` ↔ `dotsync/.config/zed`). So
the original location of everything is self-evident, names can't collide, and no
separate "where did this come from" bookkeeping is needed.

## Never guess when data genuinely diverges

Self-healing applies only when the content still matches. When a local file and its
cloud copy are *both* real and *differ*, that is a genuine conflict: the default
(`on_conflict = "fail"`) refuses and hands it to the user rather than silently
picking a winner. Choosing a side (`adopt`) is always an explicit opt-in, and even
then the loser is backed up.

## Machine-readable by default, human-readable by courtesy

Every command takes `--json` and is designed to be driven by an agent as well as a
person. Output obeys the house stream split: a command's **result** (the status
table, the `doctor` report, any `--json` document) goes to **stdout**; the narration
of a mutating run goes to **stderr**. So a pipe or redirect of stdout gets exactly
the payload.

## Per-item failure is data, not a crash

A sweep that fails on some items still exits `0` and reports the failures in its
headline (`⚠ N of M failed`) and in `--json`. A non-zero exit is reserved for "the
command could not run at all." Automation gates on the reported results (or on
`doctor`), never on a partial sweep's exit code.

## One selector namespace

A name passed to `install`/`uninstall` resolves to *either* a group or a single
mapping — never ambiguously both. Group names and mapping paths are kept in disjoint
namespaces by construction (groups take no `/` and no leading `.`; a group may not
share a name with a mapping), so a selector is never guesswork.

[`State`]: crates/dotsync-core/src/plan.rs
