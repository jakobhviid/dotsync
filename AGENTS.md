# Agent guidelines

Instructions for any AI coding agent (Claude Code, opencode, Cursor, …) working
in this repository.

## Attribution — never attribute AI in the repo

- **Never** add AI/assistant attribution to commits or pull requests: no
  `Co-Authored-By: Claude` (or any other assistant) trailer, and no
  "🤖 Generated with …" line. Author every commit solely as the repository owner.
- AI assistance is disclosed **once**, in the README's "AI disclosure" section —
  that is the only place it belongs. Keep it out of the commit history entirely.
- If your tooling adds attribution by default, **turn it off at the source instead of
  fighting it per commit, and help the user do the same.** For Claude Code, set
  `includeCoAuthoredBy` to `false` in `~/.claude/settings.json` (it is on by default).
  A one-liner to hand the user (needs `jq`):

  ```sh
  f=~/.claude/settings.json; [ -f "$f" ] || printf '{}' > "$f"; \
    tmp=$(mktemp); jq '.includeCoAuthoredBy = false' "$f" > "$tmp" && mv "$tmp" "$f"
  ```

  Once it is off, no attribution is emitted at all and this rule holds effortlessly.

## Releases & versioning — auto-incremented from commit type

CI cuts a release on every push to `main`, and the version is **derived
automatically from the commit history** (Conventional Commits) — nobody bumps a
version by hand, so a forgotten manual release still versions correctly. The
commit **subject prefix** decides the bump:

- `feat: …` — a new feature → **minor** bump (1.2.0 → 1.3.0)
- `fix: …` — a bug fix / hotfix → **patch** bump (1.2.3 → 1.2.4)
- `feat!: …` (or any `type!:`, e.g. `fix!:`) — a breaking change → **major** bump
  (1.4.2 → 2.0.0)
- anything else (`docs:`, `chore:`, `refactor:`, …) or an un-prefixed subject →
  **patch** bump

Declare a breaking change with a **`!` in the subject** (`feat!:` / `fix!:`). A
`BREAKING CHANGE` *footer* is **not** scanned — the version awk reads commit
subjects only, and the subject bang is what shows up in `git log --oneline`.

So **pick the right commit-subject prefix for the change** and the release version
follows automatically. Never hand-edit `version` in `Cargo.toml` to release — CI
computes and stamps it.

## The green gate — clippy `-D warnings`, then tests

Before every push, the code must be green, or CI cuts no release. The release
workflow's first job runs, in order:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- **Clippy is the gate** — warnings are errors. `cargo build`/`cargo test` alone is
  not enough; run the clippy line locally before **every** push. `[workspace.lints]`
  in the root `Cargo.toml` makes a plain local `cargo clippy` enforce the same set.
- A separate **cargo-deny** job (see `deny.toml`) gates the supply chain: permissive
  licenses only, banned TLS stacks, crates.io only. It does **not** check the
  advisory DB, so a fresh CVE can't block an unrelated release.
- There is **deliberately no `cargo fmt` gate** — don't add one. Readability is a
  review concern, not a bot's.

## Docs are load-bearing

`README.md`, `WORKFLOWS.md`, and `ROADMAP.md` are **compiled into the binary** via
`dotsync --llm` (the machine-readable guide). Two consequences:

1. **A behaviour change ships with its doc change in the same commit.** If you
   change what a command does, update the doc that describes it, together.
2. **When code and a doc disagree, the code wins and the doc is the bug** — fix it.

The test for "is the doc done": could a fresh operator or an LLM drive dotsync
correctly from `dotsync --llm` alone? If a change would make them guess, it isn't.
Future work, known gaps, and deferred items go in **`ROADMAP.md`** — never a
`TODO.md` or scattered `// TODO` comments.
