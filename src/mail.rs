//! Sends invite and password-reset emails over SMTP.
//!
//! Uses: `config`.
//! SRS: APP-26, APP-28, APP-38, APP-39.
//!
//! # Contract shape: pure render, thin send
//!
//! [`invite`] and [`reset`] build an [`Email`] with no I/O, so wording,
//! link and language are tested directly. [`Mailer::send`] delivers it
//! over SMTP and returns any failure, so the page can say "email not
//! sent, copy the link" (APP-26). The caller builds the link from
//! `config.base_url` and the token; this module never sees a token type.

use time::OffsetDateTime;

use crate::config::{Lang, Smtp};

/// A rendered email, ready to send. `Debug` hides `body`: it carries a
/// single-use link that works as a credential.
#[derive(Clone, PartialEq, Eq)]
pub struct Email {
    /// Recipient address.
    pub to: String,
    /// Subject line, in the email's language.
    pub subject: String,
    /// Plain-text body, in the email's language, containing the link.
    pub body: String,
}

/// Render the witness invite email: the accept link and that it expires
/// after 7 days (APP-26). The invitee has no account yet, so callers pass
/// `Lang::default()` (French, APP-38) unless they know better.
pub fn invite(to: &str, link: &str, lang: Lang) -> Email {
    let _ = (to, link, lang);
    todo!()
}

/// Render the password-reset email: the reset link and its expiry time
/// in UTC (APP-28), in the account's language (APP-39).
pub fn reset(to: &str, link: &str, expires_at: OffsetDateTime, lang: Lang) -> Email {
    let _ = (to, link, expires_at, lang);
    todo!()
}

/// SMTP sender built from [`Smtp`] settings (STARTTLS / TLS via lettre).
#[derive(Clone)]
pub struct Mailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: lettre::message::Mailbox,
}

impl Mailer {
    /// A sender for `smtp.host`, logging in with `smtp.user` and
    /// `smtp.password`, sending as `smtp.from`. Does not connect yet.
    ///
    /// # Errors
    ///
    /// [`MailError::Address`] when `smtp.from` is not an address,
    /// [`MailError::Transport`] when the transport cannot be built.
    pub fn new(smtp: &Smtp) -> Result<Mailer, MailError> {
        let _ = smtp;
        todo!()
    }

    /// Send `email` (APP-26, APP-28). Failures are returned, never
    /// swallowed; the caller logs them and tells the user (SRS 5.1).
    ///
    /// # Errors
    ///
    /// [`MailError::Address`] for a bad recipient, [`MailError::Build`]
    /// when the message cannot be built, [`MailError::Transport`] when
    /// the SMTP server cannot be reached or refuses it.
    pub async fn send(&self, email: &Email) -> Result<(), MailError> {
        let _ = email;
        todo!()
    }
}

/// Why an email was not sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailError {
    /// A sender or recipient address does not parse.
    Address(String),
    /// The message could not be assembled.
    Build(String),
    /// The SMTP server is unreachable or refused the message.
    Transport(String),
}

impl std::fmt::Debug for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = (f, &self.transport, &self.from);
        todo!()
    }
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for MailError {}

#[cfg(test)]
mod tests {
    // mail-01
    #[test] fn app_26_invite_mail_contains_link() { todo!() }
    #[test] fn app_28_reset_mail_contains_link_and_expiry() { todo!() }
    #[test] fn mail_send_failure_is_returned_to_caller() { todo!() }
    #[test] fn mail_debug_hides_link() { todo!() }

    // mail-02
    #[test] fn app_38_mail_default_french() { todo!() }
    #[test] fn app_39_mail_uses_account_language() { todo!() }
}
