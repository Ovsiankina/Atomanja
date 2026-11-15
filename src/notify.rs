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

use crate::auth::{Account, AccountId, Role};
use crate::config::{Config, Lang};
use crate::ledger::{Centimes, Change, DebtId, Ledger, TaskId, WitnessId};

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
        let task = self.task.0;
        let amount = format_amount(self.amount, self.lang);
        // DECISION: wording is short and names only `#<task>` and the
        // amount, so no message can leak a Vikunja field (APP-32).
        let (title, body) = match self.kind {
            MessageKind::ClaimedForYou => (
                "Nouvelle dette pour vous",
                format!("Tâche #{task} : {amount} vous sont dus."),
            ),
            MessageKind::Claimable => (
                "Dette à réclamer",
                format!("Tâche #{task} : {amount} à réclamer."),
            ),
            MessageKind::Sent => (
                "Paiement envoyé",
                format!("Tâche #{task} : {amount} envoyés. Confirmez la réception."),
            ),
            MessageKind::Claimed => (
                "Dette réclamée",
                format!("Tâche #{task} : la dette de {amount} a été réclamée."),
            ),
            MessageKind::Received => (
                "Paiement reçu",
                format!("Tâche #{task} : réception de {amount} confirmée."),
            ),
        };
        Text {
            title: title.to_owned(),
            body,
        }
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
    let mut out = Vec::new();
    for (i, change) in changes.iter().enumerate() {
        match *change {
            Change::DebtCreated { debt, .. } => {
                let auto_owner = changes[i + 1..].iter().find_map(|c| match *c {
                    Change::Claimed {
                        debt: d,
                        witness,
                        automatic: true,
                        ..
                    } if d == debt => Some(witness),
                    _ => None,
                });
                match auto_owner {
                    Some(owner) => push_to(
                        &mut out,
                        ledger,
                        debt,
                        MessageKind::ClaimedForYou,
                        real_witness(accounts, owner),
                    ),
                    None => {
                        for a in accounts.iter().filter(|a| a.role.is_real_witness()) {
                            push_to(&mut out, ledger, debt, MessageKind::Claimable, Some(a));
                        }
                    }
                }
            }
            Change::Claimed { debt, .. } => {
                push_to(&mut out, ledger, debt, MessageKind::Claimed, worker(accounts));
            }
            Change::MarkedSent { debt, .. } => {
                let owner = debt_row(ledger, debt)
                    .and_then(|row| ledger.debts().owner[row])
                    .and_then(|w| real_witness(accounts, w));
                push_to(&mut out, ledger, debt, MessageKind::Sent, owner);
            }
            Change::MarkedReceived { debt, .. } => {
                push_to(&mut out, ledger, debt, MessageKind::Received, worker(accounts));
            }
            // APP-37: nothing else is ever notified.
            Change::Recorded { .. }
            | Change::SeenDone { .. }
            | Change::Event { .. }
            | Change::Decided { .. }
            | Change::AmountProposed { .. }
            | Change::AmountApproved { .. }
            | Change::AmountInForce { .. } => {}
        }
    }
    out
}

/// Row of `debt` in the ledger's debt table, if it exists.
fn debt_row(ledger: &Ledger, debt: DebtId) -> Option<usize> {
    let row = usize::try_from(debt.0).ok()?;
    (row < ledger.debts().occurrence.len()).then_some(row)
}

/// Append one message about `debt` for `to`, if there is a recipient
/// and the ledger knows the debt.
// DECISION: a debt the ledger does not know (a caller bug) is skipped
// rather than panicking; a missing push must never block the action
// that triggered it (SRS 5.1).
fn push_to(
    out: &mut Vec<Message>,
    ledger: &Ledger,
    debt: DebtId,
    kind: MessageKind,
    to: Option<&Account>,
) {
    let (Some(to), Some(row)) = (to, debt_row(ledger, debt)) else {
        return;
    };
    let occurrence = ledger.debts().occurrence[row];
    let Some(&task) = usize::try_from(occurrence.0)
        .ok()
        .and_then(|o| ledger.occurrences().task.get(o))
    else {
        return;
    };
    out.push(Message {
        to: to.id,
        lang: to.lang,
        kind,
        debt,
        task,
        amount: ledger.debts().amount[row],
    });
}

/// The account of witness `w`, only if it is a real witness (APP-20).
fn real_witness(accounts: &[Account], w: WitnessId) -> Option<&Account> {
    accounts
        .iter()
        .find(|a| a.id == AccountId(w.0) && a.role.is_real_witness())
}

/// The single worker account (APP-25).
fn worker(accounts: &[Account]) -> Option<&Account> {
    accounts.iter().find(|a| a.role == Role::Worker)
}

/// `12,50 CHF` in French, `12.50 CHF` in English.
fn format_amount(amount: Centimes, lang: Lang) -> String {
    let sep = match lang {
        Lang::Fr => ',',
        Lang::En => '.',
    };
    format!("{}{sep}{:02} CHF", amount.0 / 100, amount.0 % 100)
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
    use super::{messages, Message, MessageKind};
    use crate::auth::{Account, AccountId, PasswordHash, Role};
    use crate::config::Lang;
    use crate::ledger::{
        Centimes, Change, DebtId, EventKind, Ledger, OccurrenceId, Outcome, TaskId, Timestamp,
        WitnessId,
    };

    const WORKER: AccountId = AccountId(1);
    const ADMIN: AccountId = AccountId(2);
    const ALICE: AccountId = AccountId(3);
    const BOB: AccountId = AccountId(4);
    const TASK: TaskId = TaskId(42);
    const DEBT: DebtId = DebtId(0);
    const AMOUNT: Centimes = Centimes(1250);

    fn account(id: AccountId, role: Role, lang: Lang) -> Account {
        Account {
            id,
            email: format!("u{}@example.test", id.0),
            password_hash: PasswordHash(String::new()),
            lang,
            role,
        }
    }

    /// Worker, admin witness and the given real witnesses, all French.
    fn accounts(witnesses: &[AccountId]) -> Vec<Account> {
        let mut v = vec![
            account(WORKER, Role::Worker, Lang::Fr),
            account(ADMIN, Role::Admin, Lang::Fr),
        ];
        v.extend(witnesses.iter().map(|&w| account(w, Role::Witness, Lang::Fr)));
        v
    }

    fn witness(a: AccountId) -> WitnessId {
        WitnessId(a.0)
    }

    /// Changes that end in one late occurrence of `TASK`, no debt yet.
    fn late_occurrence() -> Vec<Change> {
        let occ = OccurrenceId(0);
        vec![
            Change::Recorded { occurrence: occ, task: TASK, deadline: Timestamp(100), at: Timestamp(10) },
            Change::Decided { occurrence: occ, outcome: Outcome::Late, at: Timestamp(101) },
        ]
    }

    fn debt_created() -> Change {
        Change::DebtCreated { debt: DEBT, occurrence: OccurrenceId(0), amount: AMOUNT, at: Timestamp(101) }
    }

    fn claimed(by: AccountId, automatic: bool) -> Change {
        Change::Claimed { debt: DEBT, witness: witness(by), automatic, at: Timestamp(102) }
    }

    /// Apply `before` (earlier batches), then `batch`; return the
    /// messages for `batch` alone.
    fn run(before: &[Change], batch: &[Change], accounts: &[Account]) -> Vec<Message> {
        let mut ledger = Ledger::new();
        for c in before.iter().chain(batch) {
            ledger.apply(c);
        }
        messages(batch, &ledger, accounts)
    }

    fn msg(to: AccountId, kind: MessageKind) -> Message {
        Message { to, lang: Lang::Fr, kind, debt: DEBT, task: TASK, amount: AMOUNT }
    }

    // notify-01
    #[test]
    fn app_34_new_debt_single_witness_says_claimed_for_you() {
        let batch = [debt_created(), claimed(ALICE, true)];
        let got = run(&late_occurrence(), &batch, &accounts(&[ALICE]));
        assert_eq!(
            got,
            vec![msg(ALICE, MessageKind::ClaimedForYou), msg(WORKER, MessageKind::Claimed)]
        );
    }

    #[test]
    fn app_34_new_debt_many_witnesses_says_claimable() {
        let got = run(&late_occurrence(), &[debt_created()], &accounts(&[ALICE, BOB]));
        assert_eq!(
            got,
            vec![msg(ALICE, MessageKind::Claimable), msg(BOB, MessageKind::Claimable)]
        );
        // Zero real witnesses: claimable too, but nobody to tell.
        assert_eq!(run(&late_occurrence(), &[debt_created()], &accounts(&[])), vec![]);
    }

    #[test]
    fn app_35_sent_mark_notifies_owner_witness() {
        let mut before = late_occurrence();
        before.extend([debt_created(), claimed(BOB, false)]);
        let batch = [Change::MarkedSent { debt: DEBT, at: Timestamp(200) }];
        let got = run(&before, &batch, &accounts(&[ALICE, BOB]));
        assert_eq!(got, vec![msg(BOB, MessageKind::Sent)]);
    }

    #[test]
    fn app_36_claim_notifies_worker() {
        let mut before = late_occurrence();
        before.push(debt_created());
        let got = run(&before, &[claimed(ALICE, false)], &accounts(&[ALICE, BOB]));
        assert_eq!(got, vec![msg(WORKER, MessageKind::Claimed)]);
    }

    #[test]
    fn app_36_received_notifies_worker() {
        let mut before = late_occurrence();
        before.extend([
            debt_created(),
            claimed(ALICE, false),
            Change::MarkedSent { debt: DEBT, at: Timestamp(200) },
        ]);
        let batch = [Change::MarkedReceived { debt: DEBT, at: Timestamp(300) }];
        let got = run(&before, &batch, &accounts(&[ALICE, BOB]));
        assert_eq!(got, vec![msg(WORKER, MessageKind::Received)]);
    }

    #[test]
    fn app_37_other_changes_notify_nobody() {
        let occ = OccurrenceId(0);
        let batch = [
            Change::Recorded { occurrence: occ, task: TASK, deadline: Timestamp(100), at: Timestamp(10) },
            Change::SeenDone { occurrence: occ, done: true, at: Timestamp(20) },
            Change::Event { occurrence: occ, kind: EventKind::MovedLater, new_due: Some(Timestamp(150)), at: Timestamp(30) },
            Change::Decided { occurrence: occ, outcome: Outcome::Met, at: Timestamp(151) },
            Change::AmountProposed { amount: Centimes(2000), at: Timestamp(160) },
            Change::AmountApproved { witness: witness(ALICE), at: Timestamp(170) },
            Change::AmountInForce { amount: Centimes(2000), at: Timestamp(170) },
        ];
        assert_eq!(run(&[], &batch, &accounts(&[ALICE])), vec![]);
    }

    #[test]
    fn app_20_admin_never_notified() {
        // The admin is the only witness account: nobody real to tell.
        let got = run(&late_occurrence(), &[debt_created()], &accounts(&[]));
        assert!(got.iter().all(|m| m.to != ADMIN));
        // Even a ledger row naming the admin as owner never reaches it.
        let mut before = late_occurrence();
        before.extend([debt_created(), claimed(ADMIN, false)]);
        let batch = [Change::MarkedSent { debt: DEBT, at: Timestamp(200) }];
        assert_eq!(run(&before, &batch, &accounts(&[ALICE])), vec![]);
        let got = run(&late_occurrence(), &[debt_created(), claimed(ADMIN, true)], &accounts(&[ALICE]));
        assert!(got.iter().all(|m| m.to != ADMIN), "{got:?}");
    }

    #[test]
    fn app_32_message_text_has_no_title() {
        // A message has no field for a title; its text shows only the
        // task number and the amount.
        for kind in [
            MessageKind::ClaimedForYou,
            MessageKind::Claimable,
            MessageKind::Sent,
            MessageKind::Claimed,
            MessageKind::Received,
        ] {
            let text = msg(ALICE, kind).text();
            assert!(!text.title.is_empty());
            assert!(text.body.contains("#42"), "{text:?}");
            assert!(text.body.contains("12,50 CHF"), "{text:?}");
            let digits: String = format!("{}{}", text.title, text.body)
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            assert_eq!(digits, "421250", "only task number and amount: {text:?}");
        }
    }

    // notify-02
    #[test] fn app_33_vapid_key_roundtrip() { todo!() }
    #[test] fn notify_send_failure_is_logged_not_returned() { todo!() }
    #[test] fn notify_debug_hides_secrets() { todo!() }

    // notify-03
    #[test] fn app_38_notification_default_french() { todo!() }
    #[test] fn app_39_notification_uses_recipient_language() { todo!() }
}
