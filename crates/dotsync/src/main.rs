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
use dotsync_core::mapping::{collapse_tilde, current_os, expand_tilde, MappingsFile};
use dotsync_core::plan::{plan, Item, State};
use dotsync_core::{discovery, doctor, overview, ui};

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
    /// Show every mapping and its state on this machine.
    Status,
    /// Move an existing $HOME path into the cloud folder and symlink it back.
    Adopt {
        /// The file or directory to adopt (resolved against the current dir).
        path: PathBuf,
        /// Scope the mapping to macOS only.
        #[arg(long, conflicts_with = "linux")]
        mac: bool,
        /// Scope the mapping to Linux only.
        #[arg(long)]
        linux: bool,
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
    /// Print a shell completion script.
    Completions {
        /// The shell to generate for.
        shell: Shell,
    },
    /// Print the man page (roff).
    #[command(hide = true)]
    Man,
}

fn main() -> ExitCode {
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
            if cli.json {
                println!("{}", json!({ "error": e.to_string() }));
            } else {
                ui::err(&e.to_string());
            }
            ExitCode::FAILURE
        }
    }
}

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
        Some(Cmd::Adopt { path, mac, linux }) => cmd_adopt(path, *mac, *linux, cli.json),
        Some(Cmd::Status) => cmd_status(cli.json),
        Some(Cmd::Install {
            names,
            all,
            dry_run,
        }) => cmd_install(names, *all, *dry_run, cli.json),
        Some(Cmd::Uninstall {
            names,
            all,
            dry_run,
        }) => cmd_uninstall(names, *all, *dry_run, cli.json),
        Some(Cmd::Doctor { fix }) => cmd_doctor(*fix, cli.json),
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
    Ok((cfg, mappings))
}

fn cmd_default(json: bool) -> Result<ExitCode> {
    // Bare `dotsync`: picker on a terminal, status otherwise.
    let (cfg, mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());
    if json || !interactive() {
        return render_status(&items, &cfg, json);
    }
    overview::render(&items, &cfg);
    let outcomes = picker::run(&items)?;
    report_outcomes(&outcomes, false);
    doctor_hint(&cfg, &mappings);
    Ok(exit_from(&outcomes))
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
                ui::info(&format!("sync folder : {}", cfg.sync_dir.display()));
                ui::info(&format!("home base   : {}", cfg.home.display()));
                ui::info(&format!("config      : {}", config::config_path()?.display()));
                ui::info(&format!("mappings    : {}", mappings.mappings.len()));
            }
            Ok(ExitCode::SUCCESS)
        }
        None => {
            if json {
                println!("{}", json!({ "configured": false }));
            } else {
                ui::warn("dotsync is not configured on this machine — run `dotsync setup`");
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

    if !json {
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
        return match candidates.len() {
            1 => Ok(candidates[0].path.clone()),
            0 => bail!(
                "no cloud dotsync folder found — pass a path: `dotsync setup <dir>` \
                 (e.g. ~/Nextcloud/dotsync)"
            ),
            _ => bail!(
                "several cloud dotsync folders found — pass one: {}",
                candidates
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
            format!(
                "{}  ({}{})",
                collapse_tilde(&c.path, home),
                c.provider,
                if c.configured { ", configured" } else { "" }
            )
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

fn cmd_adopt(path: &Path, mac: bool, linux: bool, json: bool) -> Result<ExitCode> {
    let cfg = ensure_config(json)?;
    // Absolute path relative to cwd, without resolving symlinks.
    let abs = std::path::absolute(path)
        .with_context(|| format!("could not resolve {}", path.display()))?;
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
    let (mapping, outcome) = apply::adopt(&cfg, &abs, os_scope, false)?;
    mappings.upsert(mapping);
    mappings.save(&mappings_path)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&outcome_json(&outcome))?);
    } else {
        report_outcomes(std::slice::from_ref(&outcome), false);
    }
    Ok(exit_from(std::slice::from_ref(&outcome)))
}

fn select_items<'a>(items: &'a [Item], names: &[String], all: bool) -> Result<Vec<&'a Item>> {
    if !names.is_empty() {
        let mut chosen = Vec::new();
        for n in names {
            let item = items
                .iter()
                .find(|i| i.name() == n)
                .ok_or_else(|| anyhow!("no mapping named {n:?}"))?;
            chosen.push(item);
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

fn cmd_install(names: &[String], all: bool, dry_run: bool, json: bool) -> Result<ExitCode> {
    let (cfg, mappings) = load_ctx(json)?;
    let items = plan(&mappings, &cfg, current_os());

    // No explicit selection on a terminal → interactive picker.
    let use_picker = names.is_empty() && !all && !json && !dry_run && interactive();
    if use_picker {
        overview::render(&items, &cfg);
        let outcomes = picker::run(&items)?;
        report_outcomes(&outcomes, false);
        doctor_hint(&cfg, &mappings);
        return Ok(exit_from(&outcomes));
    }

    let selected = select_items(&items, names, all)?;
    if selected.is_empty() {
        bail!("nothing selected — pass mapping names, --all, or run interactively");
    }
    let outcomes: Vec<Outcome> = selected.iter().map(|i| apply::link_item(i, dry_run)).collect();
    report_outcomes(&outcomes, json);
    if !json {
        doctor_hint(&cfg, &mappings);
    }
    Ok(exit_from(&outcomes))
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
    report_outcomes(&outcomes, json);
    Ok(exit_from(&outcomes))
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

    for out in &report.fixed {
        ui::ok(&format!("{} — {} {}", out.name, out.action, out.detail));
    }
    if report.issues.is_empty() {
        ui::ok("no problems found");
    } else {
        for issue in &report.issues {
            let line = format!(
                "{} — {}{}",
                issue.name,
                issue.message,
                if issue.fixable && !fix { "  (run `dotsync doctor --fix`)" } else { "" }
            );
            match issue.level {
                doctor::Level::Warn => ui::warn(&line),
                doctor::Level::Error => ui::err(&line),
            }
        }
    }
    Ok(if report.healthy() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

/// Print a hint to run doctor if the current state has problems.
fn doctor_hint(cfg: &Config, mappings: &MappingsFile) {
    let items = plan(mappings, cfg, current_os());
    if items.iter().any(|i| i.state.is_problem()) {
        println!("\n{}", ui::dim("Some items need attention — run `dotsync doctor`."));
    }
}

fn report_outcomes(outcomes: &[Outcome], json: bool) {
    if json {
        let arr: Vec<_> = outcomes.iter().map(outcome_json).collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "results": arr })).unwrap_or_default());
        return;
    }
    if outcomes.is_empty() {
        println!("{}", ui::dim("nothing to do"));
        return;
    }
    for out in outcomes {
        let detail = if out.detail.is_empty() {
            String::new()
        } else {
            format!(" — {}", out.detail)
        };
        let line = format!("{}  {}{}", out.name, out.action, detail);
        if out.ok {
            ui::ok(&line);
        } else {
            ui::err(&line);
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

fn exit_from(outcomes: &[Outcome]) -> ExitCode {
    if outcomes.iter().all(|o| o.ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
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
        out.push_str(&format!(
            "\n\n-------------------------------- dotsync {} --------------------------------\n\n",
            sub.get_name()
        ));
        out.push_str(&sub.render_long_help().to_string());
    }
    out.push_str("\n\n================================ WORKFLOWS ================================\n\n");
    out.push_str(include_str!("../../../WORKFLOWS.md"));
    out.push_str("\n\n================================ README ================================\n\n");
    out.push_str(include_str!("../../../README.md"));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}
