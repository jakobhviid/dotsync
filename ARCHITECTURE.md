# dotsync architecture

How the code is laid out and how data flows through it. For the *behaviour* it
implements see [SPEC.md](SPEC.md); for the invariants behind the choices see
[PRINCIPLES.md](PRINCIPLES.md).

## Crate layout

A two-crate workspace — a thin CLI over a library that holds all the logic:

```
dotsync/
  Cargo.toml                     # [workspace] + [workspace.package] + release/lint profile
  crates/
    dotsync/                     # the binary: clap surface, --json/--llm, rendering
      src/main.rs                #   command dispatch → one core call → render result
      src/completions.rs         #   shell completions + man page, from the one clap def
      src/picker.rs              #   interactive multiselect (TTY only)
      tests/cli.rs               #   assert_cmd end-to-end tests
    dotsync-core/                # the library: every capability as a typed function
      src/lib.rs                 #   crate doc + the module index
      src/{config,mapping,discovery,plan,apply,doctor,fsutil,overview,ui}.rs
      tests/integration.rs       #   fixture-driven behaviour tests
```

**The CLI is thin.** Every verb in `main.rs` does three things: resolve inputs, call
one `dotsync-core` function, and render the result (a human summary, or a `--json`
document). Algorithms, filesystem mutation, and classification all live in core, so
they are unit-testable without a process; the binary reads as a command index. The
interactive `picker`/prompts live in the CLI (they are UI), but the decisions they
feed are core functions.

## The modules (core)

| Module | Responsibility |
|---|---|
| `config` | Per-machine config (`config.toml`): the sync-folder location and home base. Load/save; the git-tree guardrail. |
| `mapping` | The shared `dotsync.toml`: the `Mapping` record, the `MappingsFile` document (load/save + the version stamp), group-name rules, tilde helpers. |
| `discovery` | Find candidate cloud dotsync folders across known providers (Nextcloud, iCloud, Dropbox, OneDrive, Proton, …). |
| `plan` | **The pure analysis.** Classify each mapping's [`State`] on this machine from what's on disk. No mutation. |
| `apply` | Perform the mutations — link / unlink / adopt / restore — each returning a typed `Outcome`. Always backs up before replacing. |
| `doctor` | Diagnose problems and (with `--fix`) repair the safe ones, returning a classified `Report`. |
| `fsutil` | Filesystem primitives: symlink/tree-equality checks, mode reads/writes, secret detection, atomic copy. |
| `overview` | Render the status table (a query result → stdout). |
| `ui` | Output discipline (result→stdout / process→stderr) and the ANSI palette. |

## Data flow

Everything is computed from two inputs and the live filesystem:

```
config.toml ─┐
             ├─▶ plan::plan(mappings, cfg, os) ─▶ [Item{ mapping, target, source, state }]
dotsync.toml ┘                                        │
   (+ the real state of $HOME and the sync folder)    ├─▶ overview::render     (status)
                                                       ├─▶ picker + apply::*    (install/adopt/…)
                                                       └─▶ doctor::run          (doctor [--fix])
```

`plan` is the single classifier: it pairs each `Mapping` with its resolved `$HOME`
target and sync-folder source, inspects the filesystem, and assigns one [`State`].
`status`, the interactive picker, `install`, and `doctor` are all *views over* or
*actions driven by* that same plan — there is one notion of "what state is this
mapping in," not one per command.

## The command surface (from one clap definition)

`main.rs` defines the clap `Cli` once; `completions.rs` renders the man page and
shell completions from that same definition (so they can't drift), and `--llm`
prepends the clap long-help of every visible subcommand to the embedded docs. `man`
and `completions` are subcommands (`man` hidden); `--llm` is intercepted before clap
so it works with no subcommand. See [CLI patterns in the house guidelines].

## Where output goes

`ui` enforces the split: results (the `status` table, the `doctor` report, `config`,
`group list`, any `--json` document) → **stdout**; narration of a mutating sweep and
all warnings/errors → **stderr**. Colour is decided once per stream (`OnceLock`),
gated on that stream's terminal-ness and `NO_COLOR`. See [SPEC.md](SPEC.md#exit-codes-and-output)
and PRINCIPLES.md.

[`State`]: crates/dotsync-core/src/plan.rs
[CLI patterns in the house guidelines]: https://github.com/jakobhviid/dotsync
