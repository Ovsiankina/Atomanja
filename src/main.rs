//! Atomanja command line: `serve`, `create-worker`, `create-admin`,
//! `gen-vapid`, `seed-dev`.
//!
//! Uses: every library module.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use atomanja::auth::{self, Role};
use atomanja::config::Config;
use atomanja::ledger::Timestamp;
use atomanja::mail::Mailer;
use atomanja::notify::{Notifier, VapidKeys};
use atomanja::store::{DevDb, Kind, LedgerDb, Store, StoreError};
use atomanja::web::AppState;
use atomanja::{devdata, sync, vikunja, web};

// DECISION: subcommands are hand-parsed from std::env::args (no clap:
// no new dependency, five subcommands with at most one argument each).
const USAGE: &str = "\
usage: atomanja <subcommand> [args]

subcommands:
  serve [ADDR]          run the Vikunja poll loop and the web server
                        (ADDR defaults to 127.0.0.1:8080)
  create-worker EMAIL   create the single worker account (ledger database);
                        the password is read from stdin
  create-admin EMAIL    create the admin witness account (dev database);
                        the password is read from stdin
  gen-vapid             write a new VAPID key to ATOMANJA_VAPID_KEY
                        (never overwrites an existing file)
  seed-dev              fill the dev database with fake data
  help                  print this text

Settings come from ATOMANJA_* environment variables.";

// DECISION: default listen address when `serve` gets none; loopback so a
// fresh install is not exposed before a reverse proxy is in front.
const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

/// Why a subcommand failed. Usage errors exit 2, everything else 1.
enum Failure {
    /// Bad command line: the message, then the usage text.
    Usage(String),
    /// The subcommand ran and failed.
    Failed(String),
}

fn failed(context: &str, e: impl std::fmt::Display) -> Failure {
    Failure::Failed(format!("{context}: {e}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(msg)) => {
            eprintln!("atomanja: {msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Failed(msg)) => {
            eprintln!("atomanja: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[&str]) -> Result<(), Failure> {
    match args {
        [] => Err(Failure::Usage("no subcommand given".to_owned())),
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        ["serve"] => serve(DEFAULT_LISTEN),
        ["serve", addr] => serve(addr),
        ["create-worker", email] => create_account(email, Role::Worker),
        ["create-admin", email] => create_account(email, Role::Admin),
        ["gen-vapid"] => gen_vapid(),
        ["seed-dev"] => seed_dev(),
        [cmd @ ("serve" | "create-worker" | "create-admin" | "gen-vapid" | "seed-dev"), ..] => {
            Err(Failure::Usage(format!("wrong arguments for `{cmd}`")))
        }
        [cmd, ..] => Err(Failure::Usage(format!("unknown subcommand `{cmd}`"))),
    }
}

fn load_config() -> Result<Config, Failure> {
    Config::from_env().map_err(|e| failed("configuration", e))
}

fn open<K: Kind>(path: &Path) -> Result<Store<K>, Failure> {
    Store::<K>::open(path).map_err(|e| failed(&format!("cannot open {}", path.display()), e))
}

fn now() -> Timestamp {
    Timestamp(time::OffsetDateTime::now_utc().unix_timestamp())
}

// --- serve ---

fn serve(addr: &str) -> Result<(), Failure> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| Failure::Usage(format!("bad listen address `{addr}`: {e}")))?;
    let config = load_config()?;
    let keys = VapidKeys::load(&config.vapid_key).map_err(|e| {
        Failure::Failed(format!(
            "cannot load the VAPID key from ATOMANJA_VAPID_KEY ({}): {e}; run `atomanja gen-vapid` first",
            config.vapid_key.display()
        ))
    })?;
    let ledger = Arc::new(Mutex::new(open::<LedgerDb>(&config.ledger_db)?));
    let dev = Arc::new(Mutex::new(open::<DevDb>(&config.dev_db)?));
    let mailer = Mailer::new(&config.smtp).map_err(|e| failed("SMTP settings", e))?;
    let client = vikunja::Client::new(&config).map_err(|e| failed("Vikunja client", e))?;
    let notifier = Arc::new(Notifier::new(keys, &config));
    let titles = Arc::new(Mutex::new(BTreeMap::new()));
    let interval = config.poll_interval;
    let state = AppState {
        config: Arc::new(config),
        ledger: Arc::clone(&ledger),
        dev,
        titles: Arc::clone(&titles),
        notifier: Arc::clone(&notifier),
        mailer,
    };

    // DECISION: probe the listen address with a std listener before the
    // sync task starts, so a busy port fails here instead of after a
    // second poller has already hit Vikunja, the shared ledger and push
    // (APP-2, DB-3). The probe is dropped and web::serve binds again: a
    // pre-bound listener would need a pub change to web::serve. The
    // window between the two binds is microseconds, not a full poll.
    std::net::TcpListener::bind(addr)
        .map_err(|e| Failure::Failed(format!("web server: cannot bind the listen address {addr}: {e}")))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| failed("cannot start the async runtime", e))?;
    runtime.block_on(async move {
        // DECISION: sync runs as its own task so a slow poll never stalls
        // the accept loop; it never returns, so the process lives as long
        // as the web server does.
        tokio::spawn(async move {
            sync::run(&client, &ledger, &titles, &*notifier, interval).await;
        });
        eprintln!("atomanja: listening on http://{addr}");
        web::serve(state, addr).await.map_err(|e| failed("web server", e))
    })
}

// --- create-worker / create-admin ---

/// Create the worker (ledger file) or the admin (dev file, DEV-3).
fn create_account(email: &str, role: Role) -> Result<(), Failure> {
    let email = email.trim();
    // DECISION: a minimal shape check (one `@` with text on both sides, no
    // whitespace); the real check is the invite/reset mail reaching it.
    if !plausible_email(email) {
        return Err(Failure::Failed(format!("`{email}` is not an email address")));
    }
    let config = load_config()?;
    let mut ledger = open::<LedgerDb>(&config.ledger_db)?;
    let mut dev = open::<DevDb>(&config.dev_db)?;

    // Login looks in the ledger file first, then the dev file (DEV-3), so
    // an email present in both would hide the admin: refuse it up front.
    let taken = |found: Result<Option<_>, StoreError>| -> Result<bool, Failure> {
        found.map(|a| a.is_some()).map_err(|e| failed("cannot read accounts", e))
    };
    if taken(ledger.account_by_email(email))? || taken(dev.account_by_email(email))? {
        return Err(Failure::Failed(format!("an account with email `{email}` already exists")));
    }
    if role == Role::Worker {
        let roles: Vec<Role> = ledger
            .accounts()
            .map_err(|e| failed("cannot read accounts", e))?
            .iter()
            .map(|a| a.role)
            .collect();
        auth::may_create_worker(&roles).map_err(|e| failed("cannot create the worker", e))?;
    }

    let password = read_password()?;
    let hash = auth::hash_password(&password).map_err(|e| failed("password rejected", e))?;
    let inserted = match role {
        Role::Admin => dev.insert_account(email, &hash, role),
        Role::Worker | Role::Witness => ledger.insert_account(email, &hash, role),
    };
    let account = inserted.map_err(|e| failed("cannot create the account", e))?;
    let (what, file) = match role {
        Role::Admin => ("admin witness", &config.dev_db),
        Role::Worker | Role::Witness => ("worker", &config.ledger_db),
    };
    println!(
        "created {what} {} (id {}) in {}",
        account.email,
        account.id.0,
        file.display()
    );
    Ok(())
}

fn plausible_email(email: &str) -> bool {
    match email.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !email.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

// DECISION: the password comes from stdin only, never from argv (argv is
// visible to every local user through `ps`). On a terminal the prompt
// goes to stderr, echo is turned off with `stty` (no unsafe, no new
// dependency) and the password is asked twice; piped input is one line.
fn read_password() -> Result<String, Failure> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return read_line(&stdin);
    }
    let first = prompt_hidden(&stdin, "password: ")?;
    let second = prompt_hidden(&stdin, "repeat password: ")?;
    if first != second {
        return Err(Failure::Failed("the two passwords differ".to_owned()));
    }
    Ok(first)
}

fn prompt_hidden(stdin: &std::io::Stdin, prompt: &str) -> Result<String, Failure> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let echo_off = stty("-echo");
    let line = read_line(stdin);
    if echo_off {
        stty("echo");
    }
    eprintln!();
    line
}

/// Run `stty <arg>` on the controlling terminal; `true` if it worked.
fn stty(arg: &str) -> bool {
    std::process::Command::new("stty")
        .arg(arg)
        .stdin(std::process::Stdio::inherit())
        .status()
        .is_ok_and(|s| s.success())
}

/// One line from stdin, without its line ending (other whitespace is kept:
/// it is part of the password).
fn read_line(stdin: &std::io::Stdin) -> Result<String, Failure> {
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .map_err(|e| failed("cannot read the password from stdin", e))?;
    let trimmed = line.strip_suffix('\n').unwrap_or(&line);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Err(Failure::Failed("no password given on stdin".to_owned()));
    }
    Ok(trimmed.to_owned())
}

// --- gen-vapid ---

fn gen_vapid() -> Result<(), Failure> {
    // DECISION: only ATOMANJA_VAPID_KEY is needed, so the rest of the
    // configuration is not required yet (gen-vapid runs before setup is
    // complete). An existing file is never overwritten: losing the key
    // silently invalidates every browser push subscription.
    let path = std::env::var_os("ATOMANJA_VAPID_KEY")
        .filter(|p| !p.is_empty())
        .ok_or_else(|| Failure::Failed("configuration: ATOMANJA_VAPID_KEY is not set".to_owned()))?;
    let path = Path::new(&path);
    if path.exists() {
        return Err(Failure::Failed(format!(
            "{} already exists; delete it first to replace the key (existing push subscriptions stop working)",
            path.display()
        )));
    }
    let keys = VapidKeys::generate().map_err(|e| failed("cannot generate a VAPID key", e))?;
    keys.save(path)
        .map_err(|e| failed(&format!("cannot write {}", path.display()), e))?;
    println!("wrote VAPID private key to {}", path.display());
    println!("public key: {}", keys.public_key());
    Ok(())
}

// --- seed-dev ---

fn seed_dev() -> Result<(), Failure> {
    let config = load_config()?;
    let mut dev = open::<DevDb>(&config.dev_db)?;
    devdata::seed(&mut dev, now()).map_err(|e| failed("cannot seed the dev database", e))?;
    println!("seeded fake data into {}", config.dev_db.display());
    Ok(())
}
