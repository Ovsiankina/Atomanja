//! Sends invite and reset emails for the handlers (APP-26, APP-28).
//! A failure is logged and reported to the page, never undoes the action
//! (SRS 5.1).

use std::time::Duration;

use super::AppState;
use crate::mail::Email;

// DECISION: wait at most 10 s for SMTP, so a dead mail server delays the
// page by a bounded time and the page can still say "email not sent,
// copy the link" (HLD: mail failures are returned to the web page).
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Send `email`; `true` when the SMTP server accepted it.
pub(super) async fn deliver(state: &AppState, email: Email) -> bool {
    #[cfg(test)]
    record(&email);
    match tokio::time::timeout(SEND_TIMEOUT, state.mailer.send(&email)).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            eprintln!("web: email to {} not sent: {e}", email.to);
            false
        }
        Err(_) => {
            eprintln!("web: email to {} not sent: SMTP timed out", email.to);
            false
        }
    }
}

// DECISION: tests read what was handed to SMTP from this in-process
// record (the real transport cannot be pointed at a fake server: lettre's
// STARTTLS relay always uses port 587). Compiled only under `cfg(test)`.
#[cfg(test)]
static SENT: std::sync::Mutex<Vec<Email>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn record(email: &Email) {
    super::session::lock(&SENT).push(email.clone());
}

/// Every email handed to [`deliver`] for `to`, oldest first (tests only;
/// tests use distinct addresses because they run in parallel).
#[cfg(test)]
pub(super) fn sent_to(to: &str) -> Vec<Email> {
    super::session::lock(&SENT)
        .iter()
        .filter(|e| e.to == to)
        .cloned()
        .collect()
}
