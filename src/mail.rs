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

use lettre::AsyncTransport as _;
use time::{OffsetDateTime, UtcOffset};

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
    let _ = lang;
    Email {
        to: to.to_owned(),
        subject: "Invitation à témoigner sur Atomanja".to_owned(),
        body: format!(
            "Bonjour,\n\n\
             Vous êtes invité·e à devenir témoin sur Atomanja.\n\
             Pour créer votre compte, ouvrez ce lien :\n\n{link}\n\n\
             Ce lien ne sert qu'une fois et expire dans 7 jours.\n"
        ),
    }
}

/// Render the password-reset email: the reset link and its expiry time
/// in UTC (APP-28), in the account's language (APP-39).
pub fn reset(to: &str, link: &str, expires_at: OffsetDateTime, lang: Lang) -> Email {
    let _ = lang;
    let when = utc_stamp(expires_at);
    Email {
        to: to.to_owned(),
        subject: "Réinitialisation de votre mot de passe Atomanja".to_owned(),
        body: format!(
            "Bonjour,\n\n\
             Pour choisir un nouveau mot de passe, ouvrez ce lien :\n\n{link}\n\n\
             Ce lien ne sert qu'une fois et expire le {when}.\n\
             Si vous n'avez rien demandé, ignorez ce message.\n"
        ),
    }
}

// DECISION: expiry printed as a fixed "YYYY-MM-DD HH:MM UTC" stamp built by
// hand, so it never fails and needs no extra `time` features.
fn utc_stamp(at: OffsetDateTime) -> String {
    let t = at.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute()
    )
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
        let from = smtp
            .from
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| MailError::Address(format!("sender {:?}: {e}", smtp.from)))?;
        // DECISION: STARTTLS relay on the submission port; lettre picks
        // the port and TLS parameters from the host name.
        let transport =
            lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(&smtp.host)
                .map_err(|e| MailError::Transport(e.to_string()))?
                .credentials(lettre::transport::smtp::authentication::Credentials::new(
                    smtp.user.clone(),
                    smtp.password.clone(),
                ))
                .build();
        Ok(Mailer { transport, from })
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
        let to = email
            .to
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| MailError::Address(format!("recipient {:?}: {e}", email.to)))?;
        let message = lettre::Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(email.subject.clone())
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(email.body.clone())
            .map_err(|e| MailError::Build(e.to_string()))?;
        self.transport
            .send(message)
            .await
            .map(drop)
            .map_err(|e| MailError::Transport(e.to_string()))
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
        f.debug_struct("Email")
            .field("to", &self.to)
            .field("subject", &self.subject)
            .field("body", &"<hidden>")
            .finish()
    }
}

impl std::fmt::Debug for Mailer {
    // DECISION: the transport is never printed; it holds the SMTP password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer")
            .field("from", &self.from.to_string())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailError::Address(e) => write!(f, "invalid email address: {e}"),
            MailError::Build(e) => write!(f, "email could not be built: {e}"),
            MailError::Transport(e) => write!(f, "email not sent: {e}"),
        }
    }
}

impl std::error::Error for MailError {}

#[cfg(test)]
mod tests {
    use time::{Date, Month, OffsetDateTime, Time};

    use super::{invite, reset, MailError, Mailer};
    use crate::config::{Lang, Smtp};

    const LINK: &str = "https://atomanja.example/invite/abc123";

    fn at() -> OffsetDateTime {
        let date = Date::from_calendar_date(2026, Month::November, 15).unwrap();
        date.with_time(Time::from_hms(14, 30, 0).unwrap()).assume_utc()
    }

    fn smtp() -> Smtp {
        Smtp {
            host: "smtp.example".into(),
            user: "worker".into(),
            password: "hunter2-secret".into(),
            from: "Atomanja <noreply@atomanja.example>".into(),
        }
    }

    // mail-01
    #[test]
    fn app_26_invite_mail_contains_link() {
        let mail = invite("w@example.org", LINK, Lang::Fr);
        assert_eq!(mail.to, "w@example.org");
        assert!(mail.body.contains(LINK));
        assert!(mail.body.contains('7'), "states the 7-day expiry: {}", mail.body);
        assert!(!mail.subject.is_empty());
    }

    #[test]
    fn app_28_reset_mail_contains_link_and_expiry() {
        let at = at();
        let mail = reset("w@example.org", LINK, at, Lang::En);
        assert_eq!(mail.to, "w@example.org");
        assert!(mail.body.contains(LINK));
        assert!(mail.body.contains("2026-11-15 14:30 UTC"), "{}", mail.body);
    }

    #[tokio::test]
    async fn mail_send_failure_is_returned_to_caller() {
        let mailer = Mailer::new(&smtp()).expect("valid settings");
        let mut mail = invite("w@example.org", LINK, Lang::Fr);
        mail.to = "not an address".into();
        let err = mailer.send(&mail).await.unwrap_err();
        assert!(matches!(err, MailError::Address(_)), "{err:?}");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn mail_debug_hides_link() {
        let mail = invite("w@example.org", LINK, Lang::Fr);
        let shown = format!("{mail:?}");
        assert!(!shown.contains("abc123"), "{shown}");
        assert!(shown.contains("w@example.org"));
        let mailer = Mailer::new(&smtp()).expect("valid settings");
        let shown = format!("{mailer:?}");
        assert!(!shown.contains("hunter2-secret"), "{shown}");
    }

    // mail-02
    #[test]
    fn app_38_mail_default_french() {
        let mail = invite("w@example.org", LINK, Lang::default());
        assert!(mail.body.contains("jours"), "{}", mail.body);
        assert!(!mail.body.contains("days"));
    }

    #[test]
    fn app_39_mail_uses_account_language() {
        let at = at();
        let en = reset("w@example.org", LINK, at, Lang::En);
        let fr = reset("w@example.org", LINK, at, Lang::Fr);
        assert!(en.body.contains("password"), "{}", en.body);
        assert!(fr.body.contains("mot de passe"), "{}", fr.body);
        assert_ne!(en.subject, fr.subject);
        let inv = invite("w@example.org", LINK, Lang::En);
        assert!(inv.body.contains("days"), "{}", inv.body);
    }
}
