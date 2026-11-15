//! Loads every setting from environment variables at startup.
//!
//! Uses: nothing internal.
//! SRS: APP-1, APP-2, APP-38, DB-1, DEV-1.
//!
//! Variables: `ATOMANJA_DB`, `ATOMANJA_DEV_DB`, `ATOMANJA_VIKUNJA_URL`,
//! `ATOMANJA_VIKUNJA_TOKEN`, `ATOMANJA_POLL_SECS` (optional, default
//! 300), `ATOMANJA_BASE_URL`, `ATOMANJA_SMTP_HOST`, `ATOMANJA_SMTP_USER`,
//! `ATOMANJA_SMTP_PASSWORD`, `ATOMANJA_SMTP_FROM`, `ATOMANJA_VAPID_KEY`.

use std::path::PathBuf;
use std::time::Duration;

/// A user-facing language (APP-38). French is the default. Lives here
/// because every module that writes text (`web`, `mail`, `notify`)
/// already uses `config`.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    /// French, the default (APP-38).
    #[default]
    Fr = 0,
    /// English.
    En = 1,
}

impl Lang {
    /// Two-letter code, `"fr"` or `"en"`, as stored and as used in HTML `lang`.
    pub fn code(self) -> &'static str {
        todo!()
    }

    /// Parse a two-letter code. Unknown codes give `None`.
    pub fn from_code(code: &str) -> Option<Lang> {
        let _ = code;
        todo!()
    }
}

/// SMTP settings for invite and reset emails.
#[derive(Clone, PartialEq, Eq)]
pub struct Smtp {
    /// `ATOMANJA_SMTP_HOST`.
    pub host: String,
    /// `ATOMANJA_SMTP_USER`.
    pub user: String,
    /// `ATOMANJA_SMTP_PASSWORD`. Not printed by `Debug`.
    pub password: String,
    /// `ATOMANJA_SMTP_FROM`, the sender address.
    pub from: String,
}

/// All settings, loaded once at startup.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    /// `ATOMANJA_DB`: ledger database file (DB-1).
    pub ledger_db: PathBuf,
    /// `ATOMANJA_DEV_DB`: dev database file, distinct from `ledger_db` (DEV-1).
    pub dev_db: PathBuf,
    /// `ATOMANJA_VIKUNJA_URL`: Vikunja base URL (APP-1).
    pub vikunja_url: String,
    /// `ATOMANJA_VIKUNJA_TOKEN`: read-only API token (APP-1). Not
    /// printed by `Debug`.
    pub vikunja_token: String,
    /// `ATOMANJA_POLL_SECS`: poll interval, default 5 minutes (APP-2).
    pub poll_interval: Duration,
    /// `ATOMANJA_BASE_URL`: public URL, used in invite and reset links.
    pub base_url: String,
    /// SMTP settings.
    pub smtp: Smtp,
    /// `ATOMANJA_VAPID_KEY`: path of the VAPID private key file.
    pub vapid_key: PathBuf,
}

/// Why the settings could not be loaded. One error, naming the variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    Missing(&'static str),
    /// A variable is set but unusable.
    Invalid {
        /// The variable.
        var: &'static str,
        /// What is wrong with it.
        reason: String,
    },
}

impl Config {
    /// Default poll interval: 5 minutes (APP-2).
    pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_mins(5);

    /// Load from the process environment.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first missing or invalid variable.
    pub fn from_env() -> Result<Config, ConfigError> {
        todo!()
    }

    /// Load from any key-to-value source; `from_env` passes the real
    /// environment, tests pass a map. Rejects a dev path equal to the
    /// ledger path (DEV-1).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first missing or invalid variable.
    pub fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let _ = get;
        todo!()
    }
}

impl std::fmt::Debug for Smtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    #[test] fn app_2_poll_interval_defaults_to_5_minutes() { todo!() }
    #[test] fn app_38_default_lang_is_french() { todo!() }
    #[test] fn db_1_ledger_path_required() { todo!() }
    #[test] fn dev_1_dev_path_required_and_distinct_from_ledger() { todo!() }
    #[test] fn config_missing_variable_named_in_error() { todo!() }
    #[test] fn config_debug_hides_secrets() { todo!() }
}
