//! Test harness for `web`: in-memory stores, the real router bound to
//! `127.0.0.1:0`, and a small `reqwest` client that carries cookies by
//! hand (no `cookies` / `form` features on reqwest, no tower dev-dep).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use time::OffsetDateTime;

use super::{router, AppState};
use crate::auth::{self, Account, IssuedToken, PasswordHash, Role, Token, TokenKind};
use crate::config::Config;
use crate::ledger::{Centimes, Change, DebtId, OccurrenceId, Outcome, TaskId, Timestamp, WitnessId};
use crate::mail::Mailer;
use crate::notify::{Notifier, VapidKeys};
use crate::store::{DevDb, Kind, LedgerDb, Store};

/// A running server over fresh in-memory stores.
pub(crate) struct Harness {
    pub base: String,
    pub state: AppState,
    client: reqwest::Client,
}

/// One HTTP answer, body read as text.
pub(crate) struct Reply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: String,
}

impl Reply {
    /// The value of our session cookie in `Set-Cookie`, if any.
    pub fn session_cookie(&self) -> Option<String> {
        self.headers
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                let first = v.split(';').next()?.trim();
                let value = first.strip_prefix("atomanja_session=")?;
                (!value.is_empty()).then(|| value.to_owned())
            })
    }

    /// The `Location` header, if any.
    pub fn location(&self) -> Option<&str> {
        self.headers
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
    }
}

fn config(base_url: &str) -> Config {
    Config::from_source(|k| {
        Some(
            match k {
                "ATOMANJA_DB" => "ledger.db",
                "ATOMANJA_DEV_DB" => "dev.db",
                "ATOMANJA_VIKUNJA_URL" => "http://127.0.0.1:9",
                "ATOMANJA_VIKUNJA_TOKEN" => "token",
                "ATOMANJA_BASE_URL" => base_url,
                // Nothing listens there: sending fails at once, which the
                // pages must survive (SRS 5.1).
                "ATOMANJA_SMTP_HOST" => "127.0.0.1",
                "ATOMANJA_SMTP_USER" => "u",
                "ATOMANJA_SMTP_PASSWORD" => "p",
                "ATOMANJA_SMTP_FROM" => "Atomanja <noreply@atomanja.test>",
                "ATOMANJA_VAPID_KEY" => "/nonexistent/atomanja-test-vapid.key",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("test config")
}

impl Harness {
    /// Bind, build the state, serve in the background.
    pub async fn start() -> Harness {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let config = config(&base);
        let notifier = Notifier::new(VapidKeys::generate().expect("vapid"), &config);
        let mailer = Mailer::new(&config.smtp).expect("mailer");
        let state = AppState {
            config: Arc::new(config),
            ledger: Arc::new(Mutex::new(Store::<LedgerDb>::open_in_memory().expect("ledger"))),
            dev: Arc::new(Mutex::new(Store::<DevDb>::open_in_memory().expect("dev"))),
            titles: Arc::new(Mutex::new(BTreeMap::new())),
            starts: Arc::new(Mutex::new(BTreeMap::new())),
            notifier: Arc::new(notifier),
            mailer,
        };
        let app = router(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        Harness {
            base,
            state,
            client,
        }
    }

    async fn send(&self, req: reqwest::RequestBuilder, cookie: Option<&str>) -> Reply {
        let req = match cookie {
            Some(c) => req.header(reqwest::header::COOKIE, format!("atomanja_session={c}")),
            None => req,
        };
        let resp = req.send().await.expect("request");
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.text().await.expect("body");
        Reply {
            status,
            headers,
            body,
        }
    }

    /// `GET path`.
    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Reply {
        self.send(self.client.get(format!("{}{path}", self.base)), cookie)
            .await
    }

    /// `GET path` from a browser that prefers English.
    pub async fn get_english_browser(&self, path: &str) -> Reply {
        let req = self
            .client
            .get(format!("{}{path}", self.base))
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9");
        self.send(req, None).await
    }

    /// `POST path` as a plain HTML form.
    pub async fn post(&self, path: &str, cookie: Option<&str>, fields: &[(&str, &str)]) -> Reply {
        self.send(form(self.client.post(format!("{}{path}", self.base)), fields), cookie)
            .await
    }

    /// `POST path` as htmx sends it (`HX-Request: true`).
    pub async fn hx_post(
        &self,
        path: &str,
        cookie: Option<&str>,
        fields: &[(&str, &str)],
    ) -> Reply {
        let req = form(self.client.post(format!("{}{path}", self.base)), fields)
            .header("HX-Request", "true");
        self.send(req, cookie).await
    }

    /// `POST path` with a JSON body.
    pub async fn post_json(&self, path: &str, cookie: Option<&str>, json: &str) -> Reply {
        let req = self
            .client
            .post(format!("{}{path}", self.base))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(json.to_owned());
        self.send(req, cookie).await
    }

    /// Run `f` on the ledger store.
    pub fn ledger<R>(&self, f: impl FnOnce(&mut Store<LedgerDb>) -> R) -> R {
        f(&mut self.state.ledger.lock().expect("ledger lock"))
    }

    /// Run `f` on the dev store.
    pub fn dev<R>(&self, f: impl FnOnce(&mut Store<DevDb>) -> R) -> R {
        f(&mut self.state.dev.lock().expect("dev lock"))
    }

    /// Set the Vikunja start date `sync` would have stored for `task`.
    pub fn start_date(&self, task: i64, at: i64) {
        self.state
            .starts
            .lock()
            .expect("starts lock")
            .insert(TaskId(task), crate::ledger::Timestamp(at));
    }

        /// Set the title `sync` would have stored for `task`.
    pub fn title(&self, task: i64, title: &str) {
        self.state
            .titles
            .lock()
            .expect("titles lock")
            .insert(TaskId(task), title.to_owned());
    }
}

fn form(req: reqwest::RequestBuilder, fields: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    req.header(
        reqwest::header::CONTENT_TYPE,
        "application/x-www-form-urlencoded",
    )
    .body(body)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// An account with a real argon2 hash of `password`.
pub(crate) fn account_with_password<K: Kind>(
    store: &mut Store<K>,
    email: &str,
    password: &str,
    role: Role,
) -> Account {
    let hash = auth::hash_password(password).expect("hash");
    store.insert_account(email, &hash, role).expect("insert account")
}

/// An account whose hash matches no password (fast: no argon2).
pub(crate) fn account<K: Kind>(store: &mut Store<K>, email: &str, role: Role) -> Account {
    store
        .insert_account(email, &PasswordHash("not-a-hash".into()), role)
        .expect("insert account")
}

/// A session for `account`, stored in `store`; returns the cookie value.
pub(crate) fn session<K: Kind>(store: &mut Store<K>, account: &Account) -> String {
    let token = Token::generate();
    store
        .insert_token(&IssuedToken {
            token: token.clone(),
            kind: TokenKind::Session,
            account: Some(account.id),
            issued_at: OffsetDateTime::now_utc(),
            used: false,
        })
        .expect("insert session");
    token.0
}

/// A token of `kind` issued `age` ago, stored in `store`.
pub(crate) fn token<K: Kind>(
    store: &mut Store<K>,
    kind: TokenKind,
    account: Option<&Account>,
    age: time::Duration,
    used: bool,
) -> String {
    let token = Token::generate();
    store
        .insert_token(&IssuedToken {
            token: token.clone(),
            kind,
            account: account.map(|a| a.id),
            issued_at: OffsetDateTime::now_utc() - age,
            used,
        })
        .expect("insert token");
    token.0
}

/// 2026-11-14 12:00 UTC: the deadline used by the fixtures ("14 Nov").
pub(crate) const DEADLINE: i64 = 1_794_657_600;

/// Append a late occurrence of `task` with a 10 CHF debt, optionally
/// claimed by `owner`. Returns the debt id.
pub(crate) fn late_debt<K: Kind>(store: &mut Store<K>, task: i64, owner: Option<i64>) -> DebtId {
    let ledger = store.load_ledger().expect("load");
    let occurrence = OccurrenceId(u32::try_from(ledger.occurrences().task.len()).expect("small"));
    let debt = DebtId(u32::try_from(ledger.debts().occurrence.len()).expect("small"));
    let mut changes = vec![
        Change::Recorded {
            occurrence,
            task: TaskId(task),
            deadline: Timestamp(DEADLINE),
            at: Timestamp(DEADLINE - 86_400),
        },
        Change::Decided {
            occurrence,
            outcome: Outcome::Late,
            at: Timestamp(DEADLINE + 60),
        },
        Change::DebtCreated {
            debt,
            occurrence,
            amount: Centimes::DEFAULT_AMOUNT,
            at: Timestamp(DEADLINE + 60),
        },
    ];
    if let Some(w) = owner {
        changes.push(Change::Claimed {
            debt,
            witness: WitnessId(w),
            automatic: true,
            at: Timestamp(DEADLINE + 60),
        });
    }
    store.append(&changes).expect("append");
    debt
}
