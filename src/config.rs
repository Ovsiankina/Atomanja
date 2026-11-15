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
        match self {
            Lang::Fr => "fr",
            Lang::En => "en",
        }
    }

    /// Parse a two-letter code. Unknown codes give `None`.
    pub fn from_code(code: &str) -> Option<Lang> {
        // DECISION: exact lowercase match only; callers store what code() returns.
        match code {
            "fr" => Some(Lang::Fr),
            "en" => Some(Lang::En),
            _ => None,
        }
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
        Config::from_source(|k| std::env::var(k).ok())
    }

    /// Load from any key-to-value source; `from_env` passes the real
    /// environment, tests pass a map. Rejects a dev path equal to the
    /// ledger path (DEV-1).
    ///
    /// # Errors
    ///
    /// [`ConfigError`] naming the first missing or invalid variable.
    pub fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        // DECISION: an empty value counts as unset, so `VAR=` fails loudly.
        let req = |var: &'static str| -> Result<String, ConfigError> {
            get(var)
                .filter(|v| !v.is_empty())
                .ok_or(ConfigError::Missing(var))
        };

        let ledger_db = PathBuf::from(req("ATOMANJA_DB")?);
        let dev_db = PathBuf::from(req("ATOMANJA_DEV_DB")?);
        if dev_db == ledger_db {
            return Err(ConfigError::Invalid {
                var: "ATOMANJA_DEV_DB",
                reason: "must differ from ATOMANJA_DB".to_owned(),
            });
        }
        let vikunja_url = req("ATOMANJA_VIKUNJA_URL")?;
        let vikunja_token = req("ATOMANJA_VIKUNJA_TOKEN")?;
        let poll_interval = parse_poll(get("ATOMANJA_POLL_SECS"))?;
        let base_url = req("ATOMANJA_BASE_URL")?;
        let smtp = Smtp {
            host: req("ATOMANJA_SMTP_HOST")?,
            user: req("ATOMANJA_SMTP_USER")?,
            password: req("ATOMANJA_SMTP_PASSWORD")?,
            from: req("ATOMANJA_SMTP_FROM")?,
        };
        let vapid_key = PathBuf::from(req("ATOMANJA_VAPID_KEY")?);

        Ok(Config {
            ledger_db,
            dev_db,
            vikunja_url,
            vikunja_token,
            poll_interval,
            base_url,
            smtp,
            vapid_key,
        })
    }
}

/// Parse `ATOMANJA_POLL_SECS`; unset or empty gives the default.
fn parse_poll(raw: Option<String>) -> Result<Duration, ConfigError> {
    const VAR: &str = "ATOMANJA_POLL_SECS";
    let Some(raw) = raw.filter(|v| !v.is_empty()) else {
        return Ok(Config::DEFAULT_POLL_INTERVAL);
    };
    match raw.trim().parse::<u64>() {
        // DECISION: 0 is rejected; a zero interval would spin the poller.
        Ok(0) => Err(ConfigError::Invalid {
            var: VAR,
            reason: "must be at least 1 second".to_owned(),
        }),
        Ok(secs) => Ok(Duration::from_secs(secs)),
        Err(e) => Err(ConfigError::Invalid {
            var: VAR,
            reason: format!("not a whole number of seconds: {e}"),
        }),
    }
}

/// What `Debug` prints in place of a secret.
const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for Smtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Smtp")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("password", &REDACTED)
            .field("from", &self.from)
            .finish()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("ledger_db", &self.ledger_db)
            .field("dev_db", &self.dev_db)
            .field("vikunja_url", &self.vikunja_url)
            .field("vikunja_token", &REDACTED)
            .field("poll_interval", &self.poll_interval)
            .field("base_url", &self.base_url)
            .field("smtp", &self.smtp)
            .field("vapid_key", &self.vapid_key)
            .finish()
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Missing(var) => write!(f, "{var} is not set"),
            ConfigError::Invalid { var, reason } => write!(f, "{var} is invalid: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError, Lang};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    fn full() -> HashMap<&'static str, String> {
        [
            ("ATOMANJA_DB", "/data/ledger.db"),
            ("ATOMANJA_DEV_DB", "/data/dev.db"),
            ("ATOMANJA_VIKUNJA_URL", "https://vikunja.example"),
            ("ATOMANJA_VIKUNJA_TOKEN", "tok-secret-123"),
            ("ATOMANJA_BASE_URL", "https://atomanja.example"),
            ("ATOMANJA_SMTP_HOST", "smtp.example"),
            ("ATOMANJA_SMTP_USER", "mailer"),
            ("ATOMANJA_SMTP_PASSWORD", "pw-secret-456"),
            ("ATOMANJA_SMTP_FROM", "noreply@atomanja.example"),
            ("ATOMANJA_VAPID_KEY", "/data/vapid.pem"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_owned()))
        .collect()
    }

    fn load(map: &HashMap<&'static str, String>) -> Result<Config, ConfigError> {
        Config::from_source(|k| map.get(k).cloned())
    }

    #[test]
    fn app_2_poll_interval_defaults_to_5_minutes() {
        let mut map = full();
        assert_eq!(load(&map).unwrap().poll_interval, Duration::from_mins(5));
        assert_eq!(Config::DEFAULT_POLL_INTERVAL, Duration::from_mins(5));
        map.insert("ATOMANJA_POLL_SECS", "42".into());
        assert_eq!(load(&map).unwrap().poll_interval, Duration::from_secs(42));
        map.insert("ATOMANJA_POLL_SECS", "soon".into());
        assert!(matches!(
            load(&map),
            Err(ConfigError::Invalid { var: "ATOMANJA_POLL_SECS", .. })
        ));
        map.insert("ATOMANJA_POLL_SECS", "0".into());
        assert!(matches!(
            load(&map),
            Err(ConfigError::Invalid { var: "ATOMANJA_POLL_SECS", .. })
        ));
    }

    #[test]
    fn app_38_default_lang_is_french() {
        assert_eq!(Lang::default(), Lang::Fr);
        assert_eq!(Lang::Fr.code(), "fr");
        assert_eq!(Lang::En.code(), "en");
        assert_eq!(Lang::from_code("fr"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("en"), Some(Lang::En));
        assert_eq!(Lang::from_code("de"), None);
        assert_eq!(Lang::from_code(""), None);
    }

    #[test]
    fn db_1_ledger_path_required() {
        let mut map = full();
        assert_eq!(load(&map).unwrap().ledger_db, PathBuf::from("/data/ledger.db"));
        map.remove("ATOMANJA_DB");
        assert_eq!(load(&map), Err(ConfigError::Missing("ATOMANJA_DB")));
        map.insert("ATOMANJA_DB", String::new());
        assert_eq!(load(&map), Err(ConfigError::Missing("ATOMANJA_DB")));
    }

    #[test]
    fn dev_1_dev_path_required_and_distinct_from_ledger() {
        let mut map = full();
        assert_eq!(load(&map).unwrap().dev_db, PathBuf::from("/data/dev.db"));
        map.remove("ATOMANJA_DEV_DB");
        assert_eq!(load(&map), Err(ConfigError::Missing("ATOMANJA_DEV_DB")));
        map.insert("ATOMANJA_DEV_DB", "/data/ledger.db".into());
        assert!(matches!(
            load(&map),
            Err(ConfigError::Invalid { var: "ATOMANJA_DEV_DB", .. })
        ));
    }

    #[test]
    fn config_missing_variable_named_in_error() {
        for var in full().keys() {
            let mut map = full();
            map.remove(var);
            let err = load(&map).unwrap_err();
            assert_eq!(err, ConfigError::Missing(var));
            assert!(err.to_string().contains(var), "{err} should name {var}");
        }
        let mut map = full();
        map.insert("ATOMANJA_POLL_SECS", "x".into());
        assert!(load(&map).unwrap_err().to_string().contains("ATOMANJA_POLL_SECS"));
    }

    #[test]
    fn config_debug_hides_secrets() {
        let cfg = load(&full()).unwrap();
        let out = format!("{cfg:?}");
        assert!(!out.contains("tok-secret-123"), "{out}");
        assert!(!out.contains("pw-secret-456"), "{out}");
        assert!(out.contains("vikunja.example"), "{out}");
        assert!(out.contains("smtp.example"), "{out}");
        let smtp = format!("{:?}", cfg.smtp);
        assert!(!smtp.contains("pw-secret-456"), "{smtp}");
    }
}
