//! Turns ledger changes into Web Push notifications for the right
//! people, and owns the VAPID key pair.
//!
//! Uses: `config`, `ledger`, `auth`.
//! SRS: APP-33..APP-39.
//!
//! # Contract shape: pure "who gets what", thin send
//!
//! [`messages`] maps a persisted, applied batch of `ledger::Change`
//! values to [`Message`] values (recipient, kind, language) with no I/O,
//! so APP-34..APP-37 and APP-39 are tested directly. [`Message::text`]
//! renders the wording per language. [`Notifier::send`] pushes one
//! message to a recipient's devices; failures are logged and reported,
//! never returned as errors (SRS 5.1).
//!
//! Push subscriptions are this module's own [`Subscription`]: `store`
//! persists its own row type and the caller (`sync`, `web`) converts,
//! because the C4 model has no arrow between `notify` and `store`.

use std::path::Path;

use crate::auth::{Account, AccountId};
use crate::config::{Config, Lang};
use crate::ledger::{Centimes, Change, DebtId, Ledger, TaskId};

/// Which of the four notified events (APP-34..APP-36). Nothing else is
/// ever notified (APP-37).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MessageKind {
    /// To the single real witness: a new debt was claimed for you (APP-34, APP-17).
    ClaimedForYou,
    /// To every real witness: a new debt is claimable (APP-34, APP-18).
    Claimable,
    /// To the owner witness: the worker set the sent mark (APP-35).
    Sent,
    /// To the worker: a debt was claimed, automatically or by hand (APP-36).
    Claimed,
    /// To the worker: the owner witness set the received mark (APP-36).
    Received,
}

/// One notification for one account. Carries only what the witness view
/// may show: task number and amount, never a title (APP-32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    /// Recipient.
    pub to: AccountId,
    /// The recipient's language (APP-39).
    pub lang: Lang,
    /// What happened.
    pub kind: MessageKind,
    /// The debt concerned.
    pub debt: DebtId,
    /// Its task, shown as `#<id>`.
    pub task: TaskId,
    /// Its amount.
    pub amount: Centimes,
}

/// The rendered wording of a [`Message`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// Notification title.
    pub title: String,
    /// Notification body.
    pub body: String,
}

impl Message {
    /// Title and body in `self.lang` (APP-38, APP-39).
    pub fn text(&self) -> Text {
        todo!()
    }

    /// The JSON push payload the service worker displays: title, body
    /// and the URL to open.
    pub fn payload(&self) -> String {
        todo!()
    }
}

/// Who gets what for one batch of changes (APP-34..APP-37, APP-39).
///
/// `changes` is a batch already persisted and applied to `ledger`, so
/// the ledger knows each debt's task, amount and owner. `accounts` are
/// all accounts of the same database; real witnesses are those with
/// `Role::Witness` (APP-20), the worker is the one `Role::Worker`
/// account. Each message carries the recipient's `lang`. Changes other
/// than `DebtCreated`, `Claimed`, `MarkedSent` and `MarkedReceived`
/// produce nothing (APP-37). A `DebtCreated` followed in the batch by
/// its automatic `Claimed` gives `ClaimedForYou`, otherwise `Claimable`
/// to every real witness.
pub fn messages(changes: &[Change], ledger: &Ledger, accounts: &[Account]) -> Vec<Message> {
    let _ = (changes, ledger, accounts);
    todo!()
}

/// One browser push subscription (one device, APP-33). `Debug` hides
/// the keys.
#[derive(Clone, PartialEq, Eq)]
pub struct Subscription {
    /// Push service endpoint URL.
    pub endpoint: String,
    /// Browser public key `p256dh`, base64url.
    pub p256dh: String,
    /// Browser auth secret, base64url.
    pub auth: String,
}

/// The VAPID key pair (APP-33). Stored on disk as the base64url raw
/// P-256 private key at `config.vapid_key`. `Debug` hides the private
/// key.
#[derive(Clone, PartialEq, Eq)]
pub struct VapidKeys {
    private_b64: String,
}

impl VapidKeys {
    /// A fresh key pair from the OS RNG (CLI `gen-vapid`).
    ///
    /// # Errors
    ///
    /// [`NotifyError::Key`] if no valid key could be produced.
    pub fn generate() -> Result<VapidKeys, NotifyError> {
        todo!()
    }

    /// Load the private key file written by [`VapidKeys::save`].
    ///
    /// # Errors
    ///
    /// [`NotifyError::Io`] when the file cannot be read,
    /// [`NotifyError::Key`] when it holds no valid key.
    pub fn load(path: &Path) -> Result<VapidKeys, NotifyError> {
        let _ = path;
        todo!()
    }

    /// Write the private key file.
    ///
    /// # Errors
    ///
    /// [`NotifyError::Io`] when the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), NotifyError> {
        let _ = path;
        todo!()
    }

    /// The public key, base64url, as the browser's
    /// `applicationServerKey` (APP-33).
    pub fn public_key(&self) -> String {
        todo!()
    }
}

/// Outcome of one [`Notifier::send`]. Not an error type: failures are
/// logged and counted, never propagated (SRS 5.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    /// Devices that accepted the push.
    pub delivered: usize,
    /// Devices that failed for any other reason (logged).
    pub failed: usize,
    /// Endpoints the push service says are gone (404/410); the caller
    /// deletes these subscriptions.
    pub gone: Vec<String>,
}

/// Sends pushes signed with the VAPID keys.
pub struct Notifier {
    keys: VapidKeys,
    subject: String,
    client: web_push::HyperWebPushClient,
}

impl Notifier {
    // DECISION: the VAPID `sub` claim is `config.base_url`; the SRS names
    // no contact address and a URL is valid per RFC 8292.
    /// A sender using `keys`, with `config.base_url` as VAPID subject.
    pub fn new(keys: VapidKeys, config: &Config) -> Notifier {
        let _ = (keys, config);
        todo!()
    }

    /// Push `message` to every device in `subscriptions` (APP-33).
    /// Never fails: each failure is logged to stderr and counted.
    pub async fn send(&self, message: &Message, subscriptions: &[Subscription]) -> SendReport {
        let _ = (message, subscriptions);
        todo!()
    }
}

/// Why a VAPID key could not be generated, loaded or saved.
#[derive(Debug)]
pub enum NotifyError {
    /// Reading or writing the key file failed.
    Io(std::io::Error),
    /// The key bytes are not a valid P-256 private key.
    Key(String),
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Debug for VapidKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = (f, &self.keys, &self.subject, &self.client);
        todo!()
    }
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = f;
        todo!()
    }
}

impl std::error::Error for NotifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    // notify-01
    #[test] fn app_34_new_debt_single_witness_says_claimed_for_you() { todo!() }
    #[test] fn app_34_new_debt_many_witnesses_says_claimable() { todo!() }
    #[test] fn app_35_sent_mark_notifies_owner_witness() { todo!() }
    #[test] fn app_36_claim_notifies_worker() { todo!() }
    #[test] fn app_36_received_notifies_worker() { todo!() }
    #[test] fn app_37_other_changes_notify_nobody() { todo!() }
    #[test] fn app_20_admin_never_notified() { todo!() }
    #[test] fn app_32_message_text_has_no_title() { todo!() }

    // notify-02
    #[test] fn app_33_vapid_key_roundtrip() { todo!() }
    #[test] fn notify_send_failure_is_logged_not_returned() { todo!() }
    #[test] fn notify_debug_hides_secrets() { todo!() }

    // notify-03
    #[test] fn app_38_notification_default_french() { todo!() }
    #[test] fn app_39_notification_uses_recipient_language() { todo!() }
}
