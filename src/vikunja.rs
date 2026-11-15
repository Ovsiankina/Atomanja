//! Read-only Vikunja client: fetches every task the token can see and
//! maps it to a snapshot record. Sends only GET requests.
//!
//! Uses: `config`.
//! SRS: APP-1, APP-3, APP-8.
//!
//! # Contract shape: pure parse, thin fetch
//!
//! [`parse_tasks`] and [`parse_projects`] turn one page of Vikunja JSON
//! into records, with no I/O, so they are tested on fixture strings.
//! [`fetch_all`] walks every page through a [`PageSource`]; [`Client`]
//! is the real HTTP source, tests pass a fake one. A fetch is
//! all-or-nothing: one failed page fails the whole fetch (Review
//! Focus 2).
//!
//! The records are this module's own [`Task`], not `ledger` types: the
//! C4 model has no `vikunja -> ledger` arrow, and the worker view needs
//! the title, which `ledger::Observation` does not carry. `sync` maps
//! [`Task`] to `ledger::Observation`.

use std::future::Future;

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One Vikunja task as one fetch saw it. Fields ordered largest first
/// (NFR-2). Not a hot struct: built once per poll, then mapped by `sync`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// Task title. Shown in the worker view only (APP-31, APP-32).
    pub title: String,
    /// Due date in UTC. `None` when unset: Vikunja's
    /// `0001-01-01T00:00:00Z` sentinel maps to `None`, never to a past
    /// date (APP-3, Review Focus 1).
    pub due: Option<OffsetDateTime>,
    /// Vikunja's `done_at` in UTC; `None` for the zero sentinel (APP-8).
    pub done_at: Option<OffsetDateTime>,
    /// Vikunja task id, the task number `#<id>`.
    pub id: i64,
    /// Vikunja project id.
    pub project_id: i64,
    /// Vikunja's `repeat_after`, in seconds (APP-8).
    pub repeat_after_secs: i64,
    // DECISION: raw Vikunja value (0 default, 1 monthly, 2 from current
    // date); `sync` maps it to `ledger::RepeatMode`, keeping this module
    // free of `ledger` (no C4 arrow).
    /// Vikunja's `repeat_mode`, raw (APP-8).
    pub repeat_mode: u8,
    /// Vikunja's `done` flag.
    pub done: bool,
    /// The task's project is archived (APP-3). [`parse_tasks`] leaves it
    /// `false`; [`mark_archived`] sets it from the project list.
    pub project_archived: bool,
}

/// One Vikunja project, only what APP-3 needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Project {
    /// Vikunja project id.
    pub id: i64,
    /// Vikunja's `is_archived` (APP-3).
    pub archived: bool,
}

/// Which list a page belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    /// Every task the token can see (APP-3).
    Tasks,
    /// Every project the token can see, archived ones included (APP-3).
    Projects,
}

/// One page of a paginated Vikunja list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// The raw JSON body.
    pub body: String,
    /// Vikunja's `x-pagination-total-pages` header; 1 when absent.
    pub total_pages: u32,
}

/// Where pages come from: [`Client`] over HTTP, or a fake in tests
/// (Review Focus 2).
pub trait PageSource {
    /// Fetch page `page` (1-based) of `resource`.
    ///
    /// # Errors
    ///
    /// [`VikunjaError`] when the page cannot be fetched.
    fn page(
        &self,
        resource: Resource,
        page: u32,
    ) -> impl Future<Output = Result<Page, VikunjaError>> + Send;
}

/// Parse one page of the task list into records (APP-3, APP-8). Zero
/// date sentinels map to `None`; dates are converted to UTC;
/// `project_archived` is `false`.
///
/// # Errors
///
/// [`VikunjaError::Parse`] when the body is not the expected JSON.
pub fn parse_tasks(json: &str) -> Result<Vec<Task>, VikunjaError> {
    parse_list::<RawTask>(json)?.into_iter().map(RawTask::into_task).collect()
}

/// Parse one page of the project list (APP-3).
///
/// # Errors
///
/// [`VikunjaError::Parse`] when the body is not the expected JSON.
pub fn parse_projects(json: &str) -> Result<Vec<Project>, VikunjaError> {
    Ok(parse_list::<RawProject>(json)?
        .into_iter()
        .map(|raw| Project { id: raw.id, archived: raw.is_archived })
        .collect())
}

/// Vikunja's task JSON, only the fields this module reads.
#[derive(Deserialize)]
struct RawTask {
    id: i64,
    #[serde(default)]
    title: String,
    project_id: i64,
    #[serde(default)]
    done: bool,
    due_date: Option<String>,
    done_at: Option<String>,
    #[serde(default)]
    repeat_after: i64,
    #[serde(default)]
    repeat_mode: u8,
}

impl RawTask {
    fn into_task(self) -> Result<Task, VikunjaError> {
        Ok(Task {
            due: parse_date(self.due_date.as_deref())?,
            done_at: parse_date(self.done_at.as_deref())?,
            title: self.title,
            id: self.id,
            project_id: self.project_id,
            repeat_after_secs: self.repeat_after,
            repeat_mode: self.repeat_mode,
            done: self.done,
            project_archived: false,
        })
    }
}

/// Vikunja's project JSON, only the fields this module reads.
#[derive(Deserialize)]
struct RawProject {
    id: i64,
    #[serde(default)]
    is_archived: bool,
}

// DECISION: an empty Vikunja list may come back as `null`; treat it as
// an empty page rather than a parse error.
fn parse_list<T: for<'de> Deserialize<'de>>(json: &str) -> Result<Vec<T>, VikunjaError> {
    serde_json::from_str::<Option<Vec<T>>>(json)
        .map(Option::unwrap_or_default)
        .map_err(|e| VikunjaError::Parse(e.to_string()))
}

// DECISION: any date in year 1 or earlier is the zero sentinel, whatever
// offset it is written with; `null` and `""` also mean unset.
fn parse_date(raw: Option<&str>) -> Result<Option<OffsetDateTime>, VikunjaError> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let date = OffsetDateTime::parse(raw, &Rfc3339)
        .map_err(|e| VikunjaError::Parse(format!("date {raw:?}: {e}")))?
        .to_offset(time::UtcOffset::UTC);
    Ok((date.year() > 1).then_some(date))
}

/// Set `project_archived` on every task whose project is archived in
/// `projects` (APP-3). A task whose project is not listed stays `false`.
pub fn mark_archived(tasks: &mut [Task], projects: &[Project]) {
    let archived: std::collections::HashSet<i64> =
        projects.iter().filter(|p| p.archived).map(|p| p.id).collect();
    for task in tasks {
        task.project_archived = archived.contains(&task.project_id);
    }
}

/// Fetch every page of tasks and projects from `source`, parse them and
/// mark archived projects. All-or-nothing: the first failed or
/// unparsable page fails the whole fetch, and no partial list is ever
/// returned (APP-3, Review Focus 2).
///
/// # Errors
///
/// The first [`VikunjaError`] from any page.
pub async fn fetch_all<S: PageSource + Sync>(source: &S) -> Result<Vec<Task>, VikunjaError> {
    let _ = source;
    todo!()
}

/// The real HTTP page source: Vikunja base URL, bearer token, `GET`
/// only (APP-1).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    token: String,
}

impl Client {
    /// A client for `config.vikunja_url` with `config.vikunja_token`
    /// (APP-1).
    ///
    /// # Errors
    ///
    /// [`VikunjaError::Transport`] when the HTTP client cannot be built.
    pub fn new(config: &crate::config::Config) -> Result<Client, VikunjaError> {
        let _ = config;
        todo!()
    }

    /// The request for page `page` of `resource`: always `GET`, with the
    /// bearer token (APP-1). Every request this client sends is built
    /// here, so a test on it covers the client.
    ///
    /// # Errors
    ///
    /// [`VikunjaError::Transport`] when the URL cannot be built.
    pub fn request(&self, resource: Resource, page: u32) -> Result<reqwest::Request, VikunjaError> {
        let _ = (resource, page);
        todo!()
    }
}

impl PageSource for Client {
    async fn page(&self, resource: Resource, page: u32) -> Result<Page, VikunjaError> {
        let _ = (resource, page);
        todo!()
    }
}

/// Why a fetch failed. Any error means "no snapshot this poll" (SRS 5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VikunjaError {
    /// Vikunja unreachable, timed out, or the request could not be built.
    Transport(String),
    /// Vikunja answered with a non-success status (401/403 = bad token).
    Status(u16),
    /// The body is not the expected JSON.
    Parse(String),
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = (f, &self.http, &self.base_url, &self.token);
        todo!()
    }
}

impl std::fmt::Display for VikunjaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VikunjaError::Transport(why) => write!(f, "vikunja unreachable: {why}"),
            VikunjaError::Status(code) => write!(f, "vikunja answered HTTP {code}"),
            VikunjaError::Parse(why) => write!(f, "vikunja sent unexpected JSON: {why}"),
        }
    }
}

impl std::error::Error for VikunjaError {}

#[cfg(test)]
mod tests {
    use super::*;

    // DECISION: `time` has no `macros` feature here (no dep change), so
    // test dates are built from calendar parts.
    fn utc(y: i32, m: u8, d: u8, h: u8, min: u8) -> OffsetDateTime {
        let month = time::Month::try_from(m).expect("month");
        time::Date::from_calendar_date(y, month, d)
            .and_then(|date| date.with_hms(h, min, 0))
            .expect("valid date")
            .assume_utc()
    }

    fn task_json(extra: &str) -> String {
        format!(
            r#"[{{"id":42,"title":"Pay rent","project_id":7,"done":false,
                "due_date":"2026-12-01T10:00:00+02:00",
                "done_at":"0001-01-01T00:00:00Z",
                "repeat_after":0,"repeat_mode":0{extra}}}]"#
        )
    }

    fn one(json: &str) -> Task {
        let mut tasks = parse_tasks(json).expect("fixture parses");
        assert_eq!(tasks.len(), 1);
        tasks.remove(0)
    }

    // vikunja-02
    #[test]
    fn app_3_maps_due_date_utc() {
        let task = one(&task_json(""));
        assert_eq!(task.due, Some(utc(2026, 12, 1, 8, 0)));
        assert_eq!(task.due.map(OffsetDateTime::offset), Some(time::UtcOffset::UTC));
        assert_eq!(task.id, 42);
        assert_eq!(task.project_id, 7);
        assert_eq!(task.title, "Pay rent");
        assert!(!task.done);
        assert!(!task.project_archived);
    }

    #[test]
    fn vikunja_zero_due_date_means_none() {
        let json = task_json("").replace("2026-12-01T10:00:00+02:00", "0001-01-01T00:00:00Z");
        assert_eq!(one(&json).due, None);
    }

    #[test]
    fn vikunja_zero_done_at_means_none() {
        assert_eq!(one(&task_json("")).done_at, None);
        let json = task_json("").replace(
            r#""done_at":"0001-01-01T00:00:00Z""#,
            r#""done_at":"2026-11-14T23:30:00-01:00""#,
        );
        assert_eq!(one(&json).done_at, Some(utc(2026, 11, 15, 0, 30)));
    }

    #[test]
    fn app_8_maps_recurrence_fields() {
        let json = task_json("")
            .replace(r#""repeat_after":0"#, r#""repeat_after":604800"#)
            .replace(r#""repeat_mode":0"#, r#""repeat_mode":1"#)
            .replace(r#""done":false"#, r#""done":true"#);
        let task = one(&json);
        assert_eq!(task.repeat_after_secs, 604_800);
        assert_eq!(task.repeat_mode, 1);
        assert!(task.done);
        assert!(parse_tasks("{not json").is_err());
    }

    #[test]
    fn app_3_maps_archived_flag() {
        let projects = parse_projects(
            r#"[{"id":7,"title":"Home","is_archived":true},
                {"id":8,"title":"Work","is_archived":false}]"#,
        )
        .expect("fixture parses");
        assert_eq!(
            projects,
            vec![Project { id: 7, archived: true }, Project { id: 8, archived: false }]
        );
        let base = one(&task_json(""));
        let mut tasks = vec![
            base.clone(),
            Task { id: 43, project_id: 8, ..base.clone() },
            Task { id: 44, project_id: 99, ..base },
        ];
        mark_archived(&mut tasks, &projects);
        let flags: Vec<bool> = tasks.iter().map(|t| t.project_archived).collect();
        assert_eq!(flags, vec![true, false, false]);
    }

    // vikunja-03
    fn config(url: &str, token: &str) -> crate::config::Config {
        crate::config::Config {
            ledger_db: "ledger.db".into(),
            dev_db: "dev.db".into(),
            vikunja_url: url.to_owned(),
            vikunja_token: token.to_owned(),
            poll_interval: std::time::Duration::from_secs(300),
            base_url: "https://atomanja.example".to_owned(),
            smtp: crate::config::Smtp {
                host: "smtp.example".to_owned(),
                user: "u".to_owned(),
                password: "p".to_owned(),
                from: "a@example".to_owned(),
            },
            vapid_key: "vapid.pem".into(),
        }
    }

    #[test]
    fn app_1_client_builds_only_get_requests() {
        let client = Client::new(&config("https://vikunja.example/", "s3cret")).expect("client");
        for resource in [Resource::Tasks, Resource::Projects] {
            for page in [1, 3] {
                let req = client.request(resource, page).expect("request");
                assert_eq!(req.method(), reqwest::Method::GET);
                assert_eq!(
                    req.headers().get(reqwest::header::AUTHORIZATION).and_then(|v| v.to_str().ok()),
                    Some("Bearer s3cret")
                );
                assert!(req.body().is_none());
                let url = req.url();
                assert_eq!(url.host_str(), Some("vikunja.example"));
                assert!(url.path().starts_with("/api/v1/"), "{url}");
                assert!(url.query_pairs().any(|(k, v)| k == "page" && v == page.to_string()));
            }
        }
        let projects = client.request(Resource::Projects, 1).expect("request");
        assert!(projects.url().query_pairs().any(|(k, v)| k == "is_archived" && v == "true"));
    }

    struct FakeSource {
        pages: std::collections::HashMap<(Resource, u32), Result<Page, VikunjaError>>,
    }

    impl PageSource for FakeSource {
        async fn page(&self, resource: Resource, page: u32) -> Result<Page, VikunjaError> {
            self.pages
                .get(&(resource, page))
                .cloned()
                .unwrap_or(Err(VikunjaError::Status(404)))
        }
    }

    fn page(body: &str, total_pages: u32) -> Result<Page, VikunjaError> {
        Ok(Page { body: body.to_owned(), total_pages })
    }

    fn healthy() -> FakeSource {
        let t1 = task_json("");
        let t2 = task_json("").replace(r#""id":42"#, r#""id":43"#).replace(r#""project_id":7"#, r#""project_id":8"#);
        FakeSource {
            pages: [
                ((Resource::Tasks, 1), page(&t1, 2)),
                ((Resource::Tasks, 2), page(&t2, 2)),
                ((Resource::Projects, 1), page(r#"[{"id":7,"is_archived":true}]"#, 2)),
                ((Resource::Projects, 2), page(r#"[{"id":8,"is_archived":false}]"#, 2)),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[tokio::test]
    async fn vikunja_failed_page_fails_whole_fetch() {
        let tasks = fetch_all(&healthy()).await.expect("all pages ok");
        let got: Vec<(i64, bool)> = tasks.iter().map(|t| (t.id, t.project_archived)).collect();
        assert_eq!(got, vec![(42, true), (43, false)]);

        let failures = [
            ((Resource::Tasks, 2), Err(VikunjaError::Transport("timeout".to_owned()))),
            ((Resource::Projects, 2), Err(VikunjaError::Status(500))),
            ((Resource::Tasks, 2), page("{broken", 2)),
        ];
        for (key, failure) in failures {
            let mut source = healthy();
            source.pages.insert(key, failure);
            assert!(fetch_all(&source).await.is_err(), "{key:?} must fail the fetch");
        }
    }

    #[test]
    fn vikunja_client_debug_hides_token() {
        let client = Client::new(&config("https://vikunja.example", "s3cret")).expect("client");
        let shown = format!("{client:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(shown.contains("vikunja.example"), "{shown}");
        assert!(!VikunjaError::Status(401).to_string().is_empty());
    }
}
