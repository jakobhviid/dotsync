//! dotsync — sync user-level config between machines through a cloud folder.
//!
//! This is the thin CLI layer; all logic lives in `dotsync-core`. Each command
//! resolves config, calls into core, and renders the result as either a human
//! summary or (`--json`) a machine-readable document.
//!
//! Config is lazy: the first time a command needs a sync folder and none is
//! configured, dotsync auto-discovers candidates and asks (on a terminal), then
//! persists and continues — so `setup` is rarely needed explicitly.

mod completions;
mod picker;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use serde_json::json;

use dotsync_core::apply::{self, Outcome};
use dotsync_core::config::{self, Config};
use dotsync_core::mapping::{self, collapse_tilde, current_os, expand_tilde, MappingsFile};
use dotsync_core::plan::{plan, Item, State};
use dotsync_core::{discovery, doctor, journal, overview, ui};

const REPO_URL: &str = "https://github.com/jakobhviid/dotsync";

#[derive(Parser)]
#[command(
    name = "dotsync",
    version,
    about = "Sync user-level config between machines through a cloud folder, using symlinks.",
    disable_help_subcommand = true
)]
struct Cli {
    /// Emit machine-readable JSON instead of human output.
    #[arg(long, global = true)]
    json: bool,

    /// Print the full LLM-readable guide (every command + workflows) and exit.
    #[arg(long, global = true)]
    llm: bool,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Provision this machine: pick the cloud folder and install completions.
    ///
    /// Usually unnecessary — running any command on a fresh machine offers this
    /// automatically. Use it to re-point the sync folder or reinstall completions.
    Setup {
        /// The dotsync folder inside your cloud provider. Omit to auto-discover.
        dir: Option<PathBuf>,
        /// The home base targets are relative to (default: $HOME).
        #[arg(long)]
        home: Option<PathBuf>,
        /// Shell to install completions for (default: detected from $SHELL).
        #[arg(long)]
        shell: Option<Shell>,
    },
    /// Show the resolved per-machine configuration.
    Config,
    /// Manage groups (list, rename, move a mapping, remove a group).
    Group {
        #[command(subcommand)]
        action: GroupCmd,
    },
    /// Show every mapping and its state on this machine.
    Status,
    /// Move existing $HOME paths into the cloud folder and symlink them back.
    Adopt {
        /// Files or directories to adopt (resolved against the current dir).
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Put the adopted paths in this group. Omit to pick one interactively.
        #[arg(long)]
        group: Option<String>,
        /// Scope the mappings to macOS only.
        #[arg(long, conflicts_with = "linux")]
        mac: bool,
        /// Scope the mappings to Linux only.
        #[arg(long)]
        linux: bool,
        /// Show what would happen without moving anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Link mappings on this machine (interactive picker with no args).
    Install {
        /// Mapping names to link. Omit for the picker, or use --all.
        names: Vec<String>,
        /// Link everything applicable to this OS.
        #[arg(long)]
        all: bool,
        /// Show what would happen without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// On a conflict, let the cloud copy win and back the local file up to
        /// `<path>.bak` (a one-shot `on_conflict = "adopt"` for this run).
        #[arg(long)]
        adopt: bool,
    },
    /// Stop syncing one or more mappings everywhere: restore them to $HOME (cloud
    /// copies kept) and remove them from dotsync.toml (affects every machine).
    Unadopt {
        /// Mapping names to un-adopt.
        #[arg(required = true)]
        names: Vec<String>,
        /// Show what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Proceed without the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Remove dotsync's symlinks on this machine (cloud copies stay).
    Uninstall {
        /// Mapping names to unlink. Omit with --all to remove all.
        names: Vec<String>,
        /// Unlink everything on this machine.
        #[arg(long)]
        all: bool,
        /// Show what would happen without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Health check: find and (with --fix) repair problems.
    Doctor {
        /// Repair the safe cases (relink clobbers, re-assert secret modes).
        #[arg(long)]
        fix: bool,
    },
    /// Revert the most recent destructive run (adopt / install --adopt / unadopt /
    /// group remove).
    ///
    /// Reverses that run's changes on this machine, skipping anything you've
    /// altered since (it never clobbers). Because it re-adds or drops mappings that
    /// propagate to every machine, it asks first (or pass --yes). Use --list to see
    /// recent runs, --dry-run to preview.
    Undo {
        /// List recent undoable runs instead of reverting.
        #[arg(long)]
        list: bool,
        /// Show what undo would do without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Proceed without the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Print a shell completion script.
    Completions {
        /// The shell to generate for.
        shell: Shell,
    },
    /// Print the man page (roff).
    #[command(hide = true)]
    Man,
}

#[derive(Subcommand)]
enum GroupCmd {
    /// List groups and their members.
    List,
    /// Rename a group (relabels all its members; merges if the name exists).
    Rename { old: String, new: String },
    /// Move a mapping (by its path name) into a group.
    #[command(visible_alias = "mv")]
    Move { path: String, group: String },
    /// Stop managing a group: restore its files to $HOME, keep the cloud copies,
    /// and remove it from dotsync.toml (affects every machine).
    #[command(visible_alias = "rm")]
    Remove {
        /// The group to remove.
        name: String,
        /// Show what would happen without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Proceed without the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> ExitCode {
    // Restore the default SIGPIPE handler so piping long output into `head`/`less`
    // and quitting terminates dotsync quietly instead of panicking on a broken
    // pipe (Rust ignores SIGPIPE by default). See `reset_sigpipe`.
    reset_sigpipe();

    // `--llm` is a documentation flag like `--help`: works from anywhere, needs
    // no subcommand, so intercept it before clap enforces one.
    if std::env::args().skip(1).any(|a| a == "--llm") {
        print!("{}", llm_guide());
        return ExitCode::SUCCESS;
    }

    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(e) => {
            // `{e:#}` prints the whole anyhow context chain, not just the top line.
            if cli.json {
                println!("{}", json!({ "error": format!("{e:#}") }));
            } else {
                ui::err(&format!("{e:#}"));
            }
            ExitCode::FAILURE
        }
    }
}

/// Restore `SIGPIPE` to its default (terminate) disposition on Unix. The Rust
/// runtime sets it to `SIG_IGN`, which turns a closed downstream pipe into an
/// `ErrorKind::BrokenPipe` panic deep in a `println!`; restoring the default
/// makes `dotsync status | head` exit silently like every other CLI.
#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: resetting a signal disposition to the OS default before any threads
    // or output exist is the standard idiom; there is nothing to race with.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

fn run(cli: &Cli) -> Result<ExitCode> {
    match &cli.cmd {
        Some(Cmd::Completions { shell }) => {
            completions::print_completions(*shell);
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Man) => {
            completions::print_man()?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Setup { dir, home, shell }) => {
            provision(dir.clone(), home.clone(), *shell, cli.json).map(|_| ExitCode::SUCCESS)
        }
        Some(Cmd::Config) => cmd_config(cli.json),
        Some(Cmd::Group { action }) => cmd_group(action, cli.json),
        Some(Cmd::Adopt {
            paths,
            group,
            mac,
            linux,
            dry_run,
        }) => cmd_adopt(paths, group.clone(), *mac, *linux, *dry_run, cli.json),
        Some(Cmd::Status) => cmd_status(cli.json),
        Some(Cmd::Install {
            names,
            all,
            dry_run,
            adopt,
        }) => cmd_install(names, *all, *dry_run, *adopt, cli.json),
        Some(Cmd::Uninstall {
            names,
            all,
            dry_run,
        }) => cmd_uninstall(names, *all, *dry_run, cli.json),
        Some(Cmd::Unadopt {
            names,
            dry_run,
            yes,
        }) => cmd_unadopt(names, *dry_run, *yes, cli.json),
        Some(Cmd::Doctor { fix }) => cmd_doctor(*fix, cli.json),
        Some(Cmd::Undo { list, dry_run, yes }) => cmd_undo(*list, *dry_run, *yes, cli.json),
        None => cmd_default(cli.json),
    }
}

/// Whether we can prompt (both stdin and stdout are a terminal).
fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Get the machine's config, auto-provisioning on first interactive use.
fn ensure_config(json: bool) -> Result<Config> {
    if let Some(cfg) = config::load()? {
        return Ok(cfg);
    }
    if json || !interactive() {
        bail!("dotsync is not configured on this machine — run `dotsync setup`");
    }
    ui::info("No dotsync folder configured yet — let's find it.");
    provision(None, None, None, json)
}

/// Load config (auto-provisioning if needed) and the shared mappings file.
fn load_ctx(json: bool) -> Result<(Config, MappingsFile)> {
    let cfg = ensure_config(json)?;
    let mappings = MappingsFile::load(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
    warn_if_skewed(&mappings);
    Ok((cfg, mappings))
}

/// Warn (to stderr) when the shared `dotsync.toml` was written by a newer dotsync
/// than this build — cross-machine version skew. Per the house rules this only
/// *warns*: it never prompts, blocks, or auto-upgrades, so it's safe under `--json`
/// (stderr) and on a non-TTY.
fn warn_if_skewed(mappings: &MappingsFile) {
    if let Some(newer) = mappings.newer_writer() {
        ui::warn(&format!(
            "dotsync.toml was written by a newer dotsync ({newer}); this machine runs {}. \
             Upgrade with `brew upgrade dotsync` so newer entries are understood.",
            env!("CARGO_PKG_VERSION")
        ));
    }
}

fn cmd_default(json: bool) -> Result<ExitCode> {
    // Bare `dotsync`: picker on a terminal, status otherwise.
    let (cfg, mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    if json || !interactive() {
        return render_status(&items, &cfg, json);
    }
    overview::render(&items, &cfg);
    let outcomes = picker::run(&items, &cfg.home)?;
    record_undo("install", &outcomes);
    report_outcomes(&outcomes, &cfg.home, false);
    doctor_hint(&cfg, &mappings);
    Ok(ExitCode::SUCCESS)
}

fn cmd_status(json: bool) -> Result<ExitCode> {
    let (cfg, mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    render_status(&items, &cfg, json)
}

fn render_status(items: &[Item], cfg: &Config, json: bool) -> Result<ExitCode> {
    if json {
        println!("{}", serde_json::to_string_pretty(&overview::to_json(items, cfg))?);
    } else {
        overview::render(items, cfg);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_config(json: bool) -> Result<ExitCode> {
    match config::load()? {
        Some(cfg) => {
            let mappings = MappingsFile::load(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "configured": true,
                        "sync_dir": cfg.sync_dir.display().to_string(),
                        "home": cfg.home.display().to_string(),
                        "config": config::config_path()?.display().to_string(),
                        "mappings": mappings.mappings.len(),
                    }))?
                );
            } else {
                // `config` is a query: its result is the payload, so it goes to
                // stdout (undecorated, greppable), not through the stderr helpers.
                println!("sync folder : {}", cfg.sync_dir.display());
                println!("home base   : {}", cfg.home.display());
                println!("config      : {}", config::config_path()?.display());
                println!("mappings    : {}", mappings.mappings.len());
            }
            Ok(ExitCode::SUCCESS)
        }
        None => {
            if json {
                println!("{}", json!({ "configured": false }));
            } else {
                println!("dotsync is not configured on this machine — run `dotsync setup`");
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Provision this machine: choose the sync folder (arg, discovery, or prompt),
/// create it and `dotsync.toml` if needed, persist config, and install
/// completions. Shared by `setup` and the lazy first-run path.
fn provision(
    dir: Option<PathBuf>,
    home: Option<PathBuf>,
    shell: Option<Shell>,
    json: bool,
) -> Result<Config> {
    let home = home.map(Ok).unwrap_or_else(config::home_dir)?;
    let home = std::fs::canonicalize(&home).unwrap_or(home);

    let chosen = match dir {
        Some(d) => {
            // Expand a leading ~ that a quoted arg may have kept literal.
            expand_tilde(&d.to_string_lossy(), &home)
        }
        None => discover_or_prompt(&home, json)?,
    };

    if !chosen.exists() {
        if interactive() && !json {
            let ok = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("{} doesn't exist — create it?", chosen.display()))
                .default(true)
                .interact()?;
            if !ok {
                bail!("aborted");
            }
        }
        std::fs::create_dir_all(&chosen)
            .with_context(|| format!("could not create {}", chosen.display()))?;
    }
    let sync_dir = std::fs::canonicalize(&chosen).unwrap_or(chosen);
    config::ensure_not_in_git(&sync_dir)?;

    let mappings_path = sync_dir.join(MappingsFile::FILE_NAME);
    if !mappings_path.exists() {
        MappingsFile::default().save(&mappings_path)?;
    }

    let cfg = Config {
        sync_dir: sync_dir.clone(),
        home: home.clone(),
    };
    config::save(&cfg)?;
    let completion_note = install_completions(shell);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "configured": true,
                "sync_dir": collapse_tilde(&sync_dir, &home),
                "home": home.display().to_string(),
            }))?
        );
    } else {
        ui::ok("configured dotsync");
        ui::info(&format!("sync folder : {}", collapse_tilde(&sync_dir, &home)));
        ui::info(&format!("home base   : {}", home.display()));
        if let Some(note) = &completion_note {
            ui::info(note);
        } else {
            ui::info("completions : run `dotsync completions <shell>` to install");
        }
    }
    Ok(cfg)
}

/// Discover candidate cloud folders and (interactively) let the user pick one or
/// type a path. Non-interactively, use the sole candidate or fail with guidance.
fn discover_or_prompt(home: &Path, json: bool) -> Result<PathBuf> {
    let candidates = discovery::discover(home);

    if json || !interactive() {
        // Non-interactively, only auto-pick an already-existing folder; never
        // silently create one.
        let existing: Vec<_> = candidates.iter().filter(|c| c.exists).collect();
        return match existing.len() {
            1 => Ok(existing[0].path.clone()),
            0 => bail!(
                "no cloud dotsync folder found — pass a path: `dotsync setup <dir>` \
                 (e.g. ~/Nextcloud/Apps/dotsync)"
            ),
            _ => bail!(
                "several cloud dotsync folders found — pass one: {}",
                existing
                    .iter()
                    .map(|c| c.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    let mut labels: Vec<String> = candidates
        .iter()
        .map(|c| {
            let tag = if c.configured {
                ", configured".to_string()
            } else if c.exists {
                String::new()
            } else {
                " — create here".to_string()
            };
            format!("{}  ({}{})", collapse_tilde(&c.path, home), c.provider, tag)
        })
        .collect();
    labels.push("Enter a path manually…".to_string());

    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Which cloud folder should dotsync use?")
        .items(&labels)
        .default(0)
        .interact()?;

    if idx < candidates.len() {
        return Ok(candidates[idx].path.clone());
    }
    let input: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Path to a cloud folder for dotsync")
        .interact_text()?;
    Ok(expand_tilde(input.trim(), home))
}

/// Detect the user's shell from `$SHELL`.
fn detect_shell() -> Option<Shell> {
    let sh = std::env::var("SHELL").ok()?;
    match Path::new(&sh).file_name()?.to_string_lossy().as_ref() {
        "zsh" => Some(Shell::Zsh),
        "bash" => Some(Shell::Bash),
        "fish" => Some(Shell::Fish),
        _ => None,
    }
}

/// Install a completion script into the shell's standard user location.
/// Best-effort: returns a human note on success, `None` if it couldn't.
fn install_completions(shell: Option<Shell>) -> Option<String> {
    let shell = shell.or_else(detect_shell)?;
    let home = config::home_dir().ok()?;
    let xdg_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".local/share"));

    let (path, extra) = match shell {
        Shell::Zsh => (
            xdg_data.join("zsh/site-functions/_dotsync"),
            Some(format!(
                "if completions don't show, add to ~/.zshrc: fpath+=({}/zsh/site-functions)",
                collapse_tilde(&xdg_data, &home)
            )),
        ),
        Shell::Bash => (
            xdg_data.join("bash-completion/completions/dotsync"),
            None,
        ),
        Shell::Fish => (home.join(".config/fish/completions/dotsync.fish"), None),
        _ => return None,
    };

    std::fs::create_dir_all(path.parent()?).ok()?;
    let mut file = std::fs::File::create(&path).ok()?;
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "dotsync", &mut file);

    let mut note = format!("completions : installed for {} → {}", shell, collapse_tilde(&path, &home));
    if let Some(extra) = extra {
        note.push_str(&format!("\n         {}", extra));
    }
    Some(note)
}

fn cmd_adopt(
    paths: &[PathBuf],
    group: Option<String>,
    mac: bool,
    linux: bool,
    dry_run: bool,
    json: bool,
) -> Result<ExitCode> {
    let cfg = ensure_config(json)?;
    // Canonicalize the home base so strip_prefix is reliable.
    let home = std::fs::canonicalize(&cfg.home).unwrap_or_else(|_| cfg.home.clone());
    let cfg = Config { home, ..cfg };

    let os_scope = if mac {
        Some("mac")
    } else if linux {
        Some("linux")
    } else {
        None
    };

    let mappings_path = cfg.sync_dir.join(MappingsFile::FILE_NAME);
    let mut mappings = MappingsFile::load(&mappings_path)?;
    warn_if_skewed(&mappings);
    let existing: Vec<String> = mappings.mappings.iter().map(|m| m.name.clone()).collect();

    // Home-relative names of what we're adopting, for the group suggestion.
    let rel: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            std::path::absolute(p)
                .ok()?
                .strip_prefix(&cfg.home)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    let fallback: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let suggestion = mapping::suggest_group_name(if rel.is_empty() { &fallback } else { &rel });

    // Every mapping belongs to a group. Resolve it: explicit flag, interactive
    // pick, or (non-interactive) the auto-suggested name.
    let group = match group {
        Some(g) => {
            mapping::validate_group_name(&g)?;
            g.trim().to_string()
        }
        None if !json && interactive() => choose_group(&mappings, &suggestion, &rel)?,
        None => {
            mapping::validate_group_name(&suggestion)?;
            suggestion.clone()
        }
    };
    // A group name must not collide with a mapping name — they share the
    // `install <name>` selector space.
    mapping::ensure_free_of_mapping(&group, &mappings)?;
    // Whether we're filing into a group that already exists, so an auto-derived
    // name can't silently merge unrelated config without the user seeing it.
    let merging = mappings.groups().contains(&group);

    let mut outcomes = Vec::new();
    for path in paths {
        let abs = std::path::absolute(path)
            .with_context(|| format!("could not resolve {}", path.display()))?;
        // A new mapping's name must not collide with an existing group name.
        let name = apply::mapping_name_for(&cfg, &abs)?;
        mapping::ensure_free_of_group(&name, &mappings)?;
        let (m, outcome) = apply::adopt(&cfg, &abs, os_scope, Some(group.clone()), &existing, dry_run)?;
        mappings.upsert(m);
        outcomes.push(outcome);
    }
    if !dry_run {
        mappings.save(&mappings_path)?;
    }
    record_undo("adopt", &outcomes);

    if json {
        let arr: Vec<_> = outcomes.iter().map(outcome_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "group": group, "merged": merging, "results": arr }))?
        );
    } else {
        report_outcomes(&outcomes, &cfg.home, false);
        if outcomes.iter().any(|outcome| outcome.ok) {
            // These notes follow the sweep log on stderr, so their styling is
            // gated on stderr too (not the stdout palette).
            let note = if merging {
                ui::paint(ui::To::Err, "2", " (added to existing group)")
            } else {
                String::new()
            };
            ui::info(&format!("group: {}{}", ui::paint(ui::To::Err, "1", &group), note));
            ui::info(&ui::paint(ui::To::Err, "2", "manage with `dotsync group list/rename/move/remove`"));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Interactive group picker for `adopt`: pick an existing group or create one
/// (pre-filled with the suggested name). Groups are mandatory — no "none".
/// Existing groups are ordered by the deepest directory they share with the
/// paths being adopted, so a sibling's group floats to the top and is the
/// default; when nothing shares a directory, "New group…" is the default.
fn choose_group(mappings: &MappingsFile, suggestion: &str, rel: &[String]) -> Result<String> {
    let make_new = |prompt: &str| -> Result<String> {
        let name: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .with_initial_text(suggestion)
            .interact_text()?;
        mapping::validate_group_name(&name)?;
        mapping::ensure_free_of_mapping(&name, mappings)?;
        Ok(name.trim().to_string())
    };

    let all = mappings.groups();
    if all.is_empty() {
        return make_new("Group name");
    }

    let mut scored: Vec<(String, usize)> = all
        .iter()
        .map(|g| {
            let score = mappings
                .mappings
                .iter()
                .filter(|m| m.group.as_deref() == Some(g.as_str()))
                .flat_map(|m| rel.iter().map(move |p| shared_dir_depth(&m.name, p)))
                .max()
                .unwrap_or(0);
            (g.clone(), score)
        })
        .collect();
    scored.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
    let best = scored.first().map(|(_, s)| *s).unwrap_or(0);
    let groups: Vec<String> = scored.into_iter().map(|(g, _)| g).collect();

    let mut items = groups.clone();
    items.push("+ New group…".to_string());
    let default_idx = if best > 0 { 0 } else { groups.len() };

    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Add to which group?")
        .items(&items)
        .default(default_idx)
        .interact()?;

    if idx < groups.len() {
        Ok(groups[idx].clone())
    } else {
        make_new("New group name")
    }
}

/// Number of leading *directory* components two home-relative paths share.
fn shared_dir_depth(a: &str, b: &str) -> usize {
    fn dirs(s: &str) -> Vec<&str> {
        let mut c: Vec<&str> = s.split('/').collect();
        c.pop(); // drop the leaf
        c
    }
    dirs(a)
        .iter()
        .zip(dirs(b).iter())
        .take_while(|(x, y)| x == y)
        .count()
}

fn cmd_group(action: &GroupCmd, json: bool) -> Result<ExitCode> {
    match action {
        GroupCmd::List => cmd_group_list(json),
        GroupCmd::Rename { old, new } => cmd_group_rename(old, new, json),
        GroupCmd::Move { path, group } => cmd_group_move(path, group, json),
        GroupCmd::Remove { name, dry_run, yes } => cmd_group_remove(name, *dry_run, *yes, json),
    }
}

fn cmd_group_list(json: bool) -> Result<ExitCode> {
    let (_cfg, mappings) = load_ctx(json)?;
    let groups = mappings.groups();
    let members_of = |g: &str| -> Vec<String> {
        mappings
            .mappings
            .iter()
            .filter(|m| m.group.as_deref() == Some(g))
            .map(|m| m.name.clone())
            .collect()
    };
    if json {
        let arr: Vec<_> = groups
            .iter()
            .map(|g| json!({ "name": g, "members": members_of(g) }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "groups": arr }))?);
        return Ok(ExitCode::SUCCESS);
    }
    if groups.is_empty() {
        // `group list` is a query — its answer is the payload → stdout.
        println!("no groups yet — `dotsync adopt <path>` creates one");
        return Ok(ExitCode::SUCCESS);
    }
    for g in &groups {
        let members = members_of(g);
        println!(
            "  {}  {}",
            ui::bold(g),
            ui::dim(&format!("{} member{}", members.len(), plural(members.len())))
        );
        for m in members {
            println!("      {}", m);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_group_rename(old: &str, new: &str, json: bool) -> Result<ExitCode> {
    let (cfg, mut mappings) = load_ctx(json)?;
    let new = new.trim();
    mapping::validate_group_name(new)?;
    mapping::ensure_free_of_mapping(new, &mappings)?;
    let count = mappings
        .mappings
        .iter()
        .filter(|m| m.group.as_deref() == Some(old))
        .count();
    if count == 0 {
        bail!("no group named {old:?}");
    }
    let merging = mappings.groups().iter().any(|g| g == new);
    for m in mappings.mappings.iter_mut() {
        if m.group.as_deref() == Some(old) {
            m.group = Some(new.to_string());
        }
    }
    mappings.save(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "renamed": old, "to": new, "members": count, "merged": merging
            }))?
        );
    } else {
        let note = if merging { " (merged into existing group)" } else { "" };
        ui::ok(&format!("renamed group '{old}' → '{new}' — {count} member{}{note}", plural(count)));
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_group_move(path: &str, group: &str, json: bool) -> Result<ExitCode> {
    let (cfg, mut mappings) = load_ctx(json)?;
    let group = group.trim();
    mapping::validate_group_name(group)?;
    mapping::ensure_free_of_mapping(group, &mappings)?;
    if mappings.find(path).is_none() {
        bail!("no mapping named {path:?}");
    }
    let creating = !mappings.groups().iter().any(|g| g == group);
    for m in mappings.mappings.iter_mut() {
        if m.name == path {
            m.group = Some(group.to_string());
        }
    }
    mappings.save(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "moved": path, "group": group, "created": creating }))?
        );
    } else {
        ui::ok(&format!("moved {path} → group '{group}'"));
        if creating {
            ui::info(&format!("created new group '{group}'"));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_group_remove(name: &str, dry_run: bool, yes: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mut mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    let members: Vec<&Item> = items
        .iter()
        .filter(|i| i.mapping.group.as_deref() == Some(name))
        .collect();
    if members.is_empty() {
        bail!("no group named {name:?}");
    }
    // Only members actually linked here get restored; all get removed from config.
    let local = members
        .iter()
        .filter(|i| matches!(i.state, State::Linked | State::DanglingSelf))
        .count();

    if !dry_run && !yes {
        if json || !interactive() {
            bail!(
                "removing group {name:?} restores {local} file(s) here and removes {total} \
                 mapping(s) from dotsync.toml on ALL machines — pass --yes to confirm",
                total = members.len()
            );
        }
        let ok = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Remove group '{name}'? Restores {local} file(s) to $HOME here (cloud copies kept) \
                 and removes {} mapping(s) from dotsync.toml on ALL machines.",
                members.len()
            ))
            .default(false)
            .interact()?;
        if !ok {
            bail!("aborted");
        }
    }

    let mut outcomes = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for it in &members {
        let out = apply::restore_item(it, dry_run);
        if out.ok {
            removed.push(it.name().to_string());
        }
        outcomes.push(out);
    }
    if !dry_run {
        mappings.mappings.retain(|m| !removed.contains(&m.name));
        mappings.save(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
    }
    record_undo("group remove", &outcomes);
    report_outcomes(&outcomes, &cfg.home, json);
    Ok(ExitCode::SUCCESS)
}

fn select_items<'a>(items: &'a [Item], names: &[String], all: bool) -> Result<Vec<&'a Item>> {
    if !names.is_empty() {
        let mut chosen = Vec::new();
        for n in names {
            // A name matches either a group (expand to all members) or one mapping.
            let members: Vec<&Item> = items
                .iter()
                .filter(|i| {
                    i.mapping.group.as_deref() == Some(n.as_str())
                        && !matches!(i.state, State::Skipped | State::Missing | State::LocalOnly)
                })
                .collect();
            if !members.is_empty() {
                chosen.extend(members);
            } else if let Some(item) = items.iter().find(|i| i.name() == n) {
                chosen.push(item);
            } else {
                return Err(anyhow!("no mapping or group named {n:?}"));
            }
        }
        Ok(chosen)
    } else if all {
        Ok(items
            .iter()
            .filter(|i| !matches!(i.state, State::Skipped | State::Missing | State::LocalOnly))
            .collect())
    } else {
        Ok(Vec::new())
    }
}

fn cmd_install(names: &[String], all: bool, dry_run: bool, adopt: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mut mappings) = load_ctx(json)?;
    // `--adopt` is a one-shot conflict policy for this run: let the cloud copy
    // win, backing the local file up to `<path>.bak`. Applied in-memory only —
    // never persisted to dotsync.toml.
    if adopt {
        for m in mappings.mappings.iter_mut() {
            m.on_conflict = mapping::OnConflict::Adopt;
        }
    }
    let items = plan(&mappings, &cfg, current_os());

    // No explicit selection on a terminal → interactive picker. `--adopt` is a
    // deliberate override, so it takes the explicit path, not the picker.
    let use_picker = names.is_empty() && !all && !json && !dry_run && !adopt && interactive();
    if use_picker {
        overview::render(&items, &cfg);
        let outcomes = picker::run(&items, &cfg.home)?;
        record_undo("install", &outcomes);
        report_outcomes(&outcomes, &cfg.home, false);
        doctor_hint(&cfg, &mappings);
        return Ok(ExitCode::SUCCESS);
    }

    let selected = select_items(&items, names, all)?;
    if selected.is_empty() {
        bail!("nothing selected — pass mapping names, --all, or run interactively");
    }
    let outcomes: Vec<Outcome> = selected.iter().map(|i| apply::link_item(i, dry_run)).collect();
    record_undo("install", &outcomes);
    report_outcomes(&outcomes, &cfg.home, json);
    if !json {
        doctor_hint(&cfg, &mappings);
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_uninstall(names: &[String], all: bool, dry_run: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    let selected = if all {
        items.iter().collect()
    } else {
        select_items(&items, names, false)?
    };
    if selected.is_empty() {
        bail!("nothing selected — pass mapping names or --all");
    }
    let outcomes: Vec<Outcome> = selected
        .iter()
        .map(|i| apply::unlink_item(i, dry_run))
        .collect();
    report_outcomes(&outcomes, &cfg.home, json);
    Ok(ExitCode::SUCCESS)
}

fn cmd_unadopt(names: &[String], dry_run: bool, yes: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mut mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    // Resolve each name to a single mapping (groups go through `group remove`).
    let mut targets: Vec<&Item> = Vec::new();
    for n in names {
        match items.iter().find(|i| i.name() == n) {
            Some(it) => targets.push(it),
            None => bail!("no mapping named {n:?}"),
        }
    }

    if !dry_run && !yes {
        if json || !interactive() {
            bail!(
                "un-adopting {n} mapping(s) restores them here and removes them from dotsync.toml \
                 on ALL machines — pass --yes to confirm",
                n = targets.len()
            );
        }
        let ok = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Un-adopt {} mapping(s)? Restores them to $HOME here (cloud copies kept) and \
                 removes them from dotsync.toml on ALL machines.",
                targets.len()
            ))
            .default(false)
            .interact()?;
        if !ok {
            bail!("aborted");
        }
    }

    let mut outcomes = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for it in &targets {
        let out = apply::restore_item(it, dry_run);
        if out.ok {
            removed.push(it.name().to_string());
        }
        outcomes.push(out);
    }
    if !dry_run {
        mappings.mappings.retain(|m| !removed.contains(&m.name));
        mappings.save(&cfg.sync_dir.join(MappingsFile::FILE_NAME))?;
    }
    record_undo("unadopt", &outcomes);
    report_outcomes(&outcomes, &cfg.home, json);
    Ok(ExitCode::SUCCESS)
}

fn cmd_doctor(fix: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mappings) = load_ctx(json)?;
    let report = doctor::run(&cfg, &mappings, current_os(), fix)?;

    if json {
        let issues: Vec<_> = report
            .issues
            .iter()
            .map(|i| {
                json!({
                    "name": i.name,
                    "level": match i.level { doctor::Level::Warn => "warn", doctor::Level::Error => "error" },
                    "message": i.message,
                    "fixable": i.fixable,
                })
            })
            .collect();
        let fixed: Vec<_> = report.fixed.iter().map(outcome_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "healthy": report.healthy(),
                "issues": issues,
                "fixed": fixed,
            }))?
        );
        return Ok(if report.healthy() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
    }

    let home_str = cfg.home.to_string_lossy().into_owned();
    let short = |s: &str| s.replace(&home_str, "~");

    // doctor is a query: its whole report — the fixes it applied included — is the
    // result, so it all goes to stdout (greppable/capturable as one document).
    for out in &report.fixed {
        println!("{} fixed {}  {} {}", ui::green("✓"), ui::bold(&out.name), out.action, short(&out.detail));
    }

    let errors: Vec<_> = report.errors().collect();
    let advisories: Vec<_> = report.advisories().collect();

    // Findings go to stdout (not stderr) alongside their headers, so `doctor`
    // output is capturable/greppable as a whole — the findings *are* the output.
    if !errors.is_empty() {
        println!("\n  {}", ui::bold("Problems"));
        for issue in &errors {
            println!("  {} {}  {}", ui::red("✗"), ui::bold(&short(&issue.name)), short(&issue.message));
        }
    }
    if !advisories.is_empty() {
        println!("\n  {}", ui::bold("Advisories"));
        for issue in &advisories {
            let hint = if issue.fixable && !fix {
                ui::dim("  (dotsync doctor --fix)")
            } else {
                String::new()
            };
            println!("  {} {}  {}{}", ui::yellow("⚠"), ui::bold(&short(&issue.name)), short(&issue.message), hint);
        }
    }

    println!();
    if errors.is_empty() && advisories.is_empty() && report.fixed.is_empty() {
        println!("{} everything looks healthy", ui::green("✓"));
    } else {
        let mut parts = Vec::new();
        if !errors.is_empty() {
            parts.push(ui::red(&format!("{} problem{}", errors.len(), plural(errors.len()))));
        }
        if !advisories.is_empty() {
            parts.push(ui::yellow(&format!("{} advisor{}", advisories.len(), if advisories.len() == 1 { "y" } else { "ies" })));
        }
        if !report.fixed.is_empty() {
            parts.push(ui::green(&format!("{} fixed", report.fixed.len())));
        }
        println!("  {}", parts.join(&ui::dim(" · ")));
    }
    Ok(if report.healthy() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Print a hint to run doctor if the current state has problems.
fn doctor_hint(cfg: &Config, mappings: &MappingsFile) {
    let items = plan(mappings, cfg, current_os());
    if items.iter().any(|item| item.state.is_problem()) {
        // A trailing nudge is narration, not the command's result → stderr.
        eprintln!(
            "\n{}",
            ui::paint(ui::To::Err, "2", "Other items need attention — run `dotsync doctor`.")
        );
    }
}

/// Record a destructive sweep's reversible actions to the undo journal.
/// Best-effort: a journaling failure warns but never fails an already-successful
/// mutation — the op is done; only the ability to `undo` it is lost.
fn record_undo(command: &str, outcomes: &[Outcome]) {
    let actions: Vec<_> = outcomes.iter().filter_map(|outcome| outcome.undo.clone()).collect();
    if actions.is_empty() {
        return;
    }
    let Some(dir) = journal::default_dir() else {
        return;
    };
    if let Err(e) = journal::record(&dir, command, actions) {
        ui::warn(&format!("could not record undo journal: {e:#}"));
    }
}

fn cmd_undo(list: bool, dry_run: bool, yes: bool, json: bool) -> Result<ExitCode> {
    let Some(dir) = journal::default_dir() else {
        bail!("cannot locate a state directory (is $HOME set?) — no undo journal");
    };
    if list {
        return cmd_undo_list(&dir, json);
    }

    let cfg = config::load()?
        .ok_or_else(|| anyhow!("dotsync is not configured on this machine — nothing to undo"))?;

    let Some(run) = journal::latest(&dir) else {
        if json {
            println!("{}", json!({ "reverted": [] }));
        } else {
            ui::info("nothing to undo");
        }
        return Ok(ExitCode::SUCCESS);
    };

    // Undo re-adds/drops mappings that propagate to every machine, so confirm like
    // the other cross-machine-destructive verbs (or bail non-interactively).
    if !dry_run && !yes {
        if json || !interactive() {
            bail!(
                "undoing `{}` ({} item(s)) reverses changes and may re-add/drop mappings on ALL \
                 machines — pass --yes to confirm",
                run.command,
                run.actions.len()
            );
        }
        let confirmed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Undo the last run (`{}`, {} item(s))? Reverses those changes here and may \
                 re-add/drop mappings on every machine.",
                run.command,
                run.actions.len()
            ))
            .default(false)
            .interact()?;
        if !confirmed {
            bail!("aborted");
        }
    }

    let Some((run, outcomes)) = journal::revert(&dir, &cfg, dry_run)? else {
        ui::info("nothing to undo");
        return Ok(ExitCode::SUCCESS);
    };
    if !json {
        let label = if dry_run { "would undo" } else { "undoing" };
        ui::info(&format!("{label} `{}`", run.command));
    }
    report_outcomes(&outcomes, &cfg.home, json);
    Ok(ExitCode::SUCCESS)
}

fn cmd_undo_list(dir: &Path, json: bool) -> Result<ExitCode> {
    let runs = journal::list(dir);
    if json {
        let arr: Vec<_> = runs
            .iter()
            .map(|run| {
                json!({
                    "id": run.id.to_string(),
                    "command": run.command,
                    "items": run.actions.len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "runs": arr }))?);
        return Ok(ExitCode::SUCCESS);
    }
    if runs.is_empty() {
        println!("no undo history");
        return Ok(ExitCode::SUCCESS);
    }
    // `undo --list` is a query: its answer is the payload → stdout.
    for run in &runs {
        println!(
            "  {}  {}  {}",
            ui::dim(&undo_age(run.id)),
            ui::bold(&run.command),
            ui::dim(&format!("{} item{}", run.actions.len(), plural(run.actions.len())))
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// A coarse "N ago" for a run id (millis since the epoch) — lib-free, for `--list`.
fn undo_age(id_millis: u128) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(id_millis);
    let secs = now.saturating_sub(id_millis) / 1000;
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Render a mutating sweep's per-item outcomes. Exit-code contract: a sweep
/// **always exits `SUCCESS`** — per-item failures are surfaced here (the `⚠`
/// headline, each `✗` line) and in `--json`, never in `$?`. Non-zero is reserved
/// for "the command could not run at all" (a hard error, propagated as `Err` and
/// rendered by `main`). Callers gate on the results, not the exit status.
fn report_outcomes(outcomes: &[Outcome], home: &Path, json: bool) {
    if json {
        // The machine-readable result of a sweep is its JSON document → stdout.
        let arr: Vec<_> = outcomes.iter().map(outcome_json).collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "results": arr })).unwrap_or_default());
        return;
    }
    // A mutating sweep narrates what it did. That per-item log is *process*, not
    // result — and because it carries failures (which must never land on stdout)
    // the whole log, successes included, unifies onto stderr as one ordered
    // stream. `--json` above is the result a program should consume.
    if outcomes.is_empty() {
        eprintln!("{}", ui::paint(ui::To::Err, "2", "nothing to do"));
        return;
    }
    let home_str = home.to_string_lossy().into_owned();
    for out in outcomes {
        // Collapse long absolute home paths in the detail for readability.
        let detail = out.detail.replace(&home_str, "~");
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(" — {}", ui::paint(ui::To::Err, "2", &detail))
        };
        let glyph = if out.ok {
            ui::paint(ui::To::Err, "32", "✓")
        } else {
            ui::paint(ui::To::Err, "31", "✗")
        };
        eprintln!("{glyph} {}  {}{}", ui::paint(ui::To::Err, "1", &out.name), out.action, detail);
    }
    // Failure-aware headline: when any item failed, lead with `⚠ N of M failed`
    // (glyph *and* text, never colour alone) so partial failure stays visible even
    // when the log is piped and colour is stripped. Shown only for a multi-item
    // sweep — a lone failure is already salient on its own `✗` line above.
    if outcomes.len() > 1 {
        let ok = outcomes.iter().filter(|outcome| outcome.ok).count();
        let failed = outcomes.len() - ok;
        if failed > 0 {
            eprintln!(
                "{} {} {}",
                ui::paint(ui::To::Err, "33", &format!("⚠ {failed} of {} failed", outcomes.len())),
                ui::paint(ui::To::Err, "2", "·"),
                ui::paint(ui::To::Err, "32", &format!("{ok} ok")),
            );
        } else {
            eprintln!("{}", ui::paint(ui::To::Err, "32", &format!("✓ {ok} ok")));
        }
    }
}

fn outcome_json(out: &Outcome) -> serde_json::Value {
    json!({
        "name": out.name,
        "action": out.action,
        "ok": out.ok,
        "detail": out.detail,
    })
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// The self-contained guide printed by `--llm`.
fn llm_guide() -> String {
    let mut cmd = Cli::command();
    let mut out = String::new();
    out.push_str(&format!("dotsync {} — LLM guide\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!("Repository: {REPO_URL}\n"));
    out.push_str("This is the same reference as `man dotsync`, laid out plainly for LLM reading.\n\n");
    out.push_str("================================ COMMAND REFERENCE ================================\n\n");
    out.push_str(&cmd.render_long_help().to_string());
    for sub in cmd.get_subcommands_mut() {
        if sub.is_hide_set() {
            continue;
        }
        let subname = sub.get_name().to_string();
        out.push_str(&format!(
            "\n\n-------------------------------- dotsync {subname} --------------------------------\n\n"
        ));
        out.push_str(&sub.render_long_help().to_string());
        // Recurse one level so nested verbs (e.g. `group remove`) show their full
        // argument signatures, not just a one-line summary.
        for nested in sub.get_subcommands_mut() {
            if nested.is_hide_set() {
                continue;
            }
            let nname = nested.get_name().to_string();
            out.push_str(&format!(
                "\n\n----------------- dotsync {subname} {nname} -----------------\n\n"
            ));
            out.push_str(&nested.render_long_help().to_string());
        }
    }
    out.push_str("\n\n================================ WORKFLOWS ================================\n\n");
    out.push_str(include_str!("../../../WORKFLOWS.md"));
    out.push_str("\n\n================================ SPEC ================================\n\n");
    out.push_str(include_str!("../../../SPEC.md"));
    out.push_str("\n\n================================ README ================================\n\n");
    out.push_str(include_str!("../../../README.md"));
    out.push_str("\n\n================================ ARCHITECTURE ================================\n\n");
    out.push_str(include_str!("../../../ARCHITECTURE.md"));
    out.push_str("\n\n================================ PRINCIPLES ================================\n\n");
    out.push_str(include_str!("../../../PRINCIPLES.md"));
    out.push_str("\n\n================================ ROADMAP ================================\n\n");
    out.push_str(include_str!("../../../ROADMAP.md"));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}
