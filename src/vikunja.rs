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

use time::OffsetDateTime;

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
    let _ = json;
    todo!()
}

/// Parse one page of the project list (APP-3).
///
/// # Errors
///
/// [`VikunjaError::Parse`] when the body is not the expected JSON.
pub fn parse_projects(json: &str) -> Result<Vec<Project>, VikunjaError> {
    let _ = json;
    todo!()
}

/// Set `project_archived` on every task whose project is archived in
/// `projects` (APP-3). A task whose project is not listed stays `false`.
pub fn mark_archived(tasks: &mut [Task], projects: &[Project]) {
    let _ = (tasks, projects);
    todo!()
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
        let _ = f;
        todo!()
    }
}

impl std::error::Error for VikunjaError {}

#[cfg(test)]
mod tests {
    // vikunja-02
    #[test] fn app_3_maps_due_date_utc() { todo!() }
    #[test] fn vikunja_zero_due_date_means_none() { todo!() }
    #[test] fn vikunja_zero_done_at_means_none() { todo!() }
    #[test] fn app_8_maps_recurrence_fields() { todo!() }
    #[test] fn app_3_maps_archived_flag() { todo!() }

    // vikunja-03
    #[test] fn app_1_client_builds_only_get_requests() { todo!() }
    #[test] fn vikunja_failed_page_fails_whole_fetch() { todo!() }
    #[test] fn vikunja_client_debug_hides_token() { todo!() }
}
