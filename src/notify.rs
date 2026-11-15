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
        let text = self.text();
        // DECISION: the URL is the site root, relative, so the service
        // worker resolves it against its own origin and the root page
        // sends each role to its view.
        serde_json::json!({ "title": text.title, "body": text.body, "url": "/" }).to_string()
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
    // DECISION: the public key is derived once when the private key is
    // validated, so `public_key` cannot fail or panic later.
    public_b64: String,
}

impl VapidKeys {
    /// A fresh key pair from the OS RNG (CLI `gen-vapid`).
    ///
    /// # Errors
    ///
    /// [`NotifyError::Key`] if no valid key could be produced.
    pub fn generate() -> Result<VapidKeys, NotifyError> {
        use rand::TryRng;
        // DECISION: a random 32-byte scalar is a valid P-256 key except
        // with probability ~2^-32 (zero or >= the group order); retry a
        // few times rather than pull in a key-generation crate.
        let mut last = String::new();
        for _ in 0..4 {
            let mut bytes = [0u8; 32];
            rand::rngs::SysRng
                .try_fill_bytes(&mut bytes)
                .map_err(|e| NotifyError::Key(format!("OS random number generator failed: {e}")))?;
            match VapidKeys::from_private(base64url(&bytes)) {
                Ok(keys) => return Ok(keys),
                Err(e) => last = e.to_string(),
            }
        }
        Err(NotifyError::Key(last))
    }

    /// Load the private key file written by [`VapidKeys::save`].
    ///
    /// # Errors
    ///
    /// [`NotifyError::Io`] when the file cannot be read,
    /// [`NotifyError::Key`] when it holds no valid key.
    pub fn load(path: &Path) -> Result<VapidKeys, NotifyError> {
        let text = std::fs::read_to_string(path).map_err(NotifyError::Io)?;
        VapidKeys::from_private(text.trim().to_owned())
    }

    /// Write the private key file.
    ///
    /// # Errors
    ///
    /// [`NotifyError::Io`] when the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), NotifyError> {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        // DECISION: a new key file is created owner-read/write only.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path).map_err(NotifyError::Io)?;
        writeln!(file, "{}", self.private_b64).map_err(NotifyError::Io)
    }

    /// The public key, base64url, as the browser's
    /// `applicationServerKey` (APP-33).
    pub fn public_key(&self) -> String {
        self.public_b64.clone()
    }

    /// Validate a base64url raw private key and derive its public key.
    fn from_private(private_b64: String) -> Result<VapidKeys, NotifyError> {
        let builder = web_push::VapidSignatureBuilder::from_base64_no_sub(&private_b64)
            .map_err(|_| NotifyError::Key("not a base64url raw P-256 private key".into()))?;
        let public_b64 = base64url(&builder.get_public_key());
        Ok(VapidKeys {
            private_b64,
            public_b64,
        })
    }
}

/// Unpadded base64url (RFC 4648 section 5) of `bytes`.
// DECISION: hand-rolled, as in `auth`, instead of a base64 dependency.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..=chunk.len() {
            let sextet = (n >> (18 - 6 * i)) & 0x3f;
            out.push(char::from(ALPHABET[sextet as usize]));
        }
    }
    out
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
        Notifier {
            keys,
            subject: config.base_url.clone(),
            client: web_push::HyperWebPushClient::new(),
        }
    }

    /// Push `message` to every device in `subscriptions` (APP-33).
    /// Never fails: each failure is logged to stderr and counted.
    pub async fn send(&self, message: &Message, subscriptions: &[Subscription]) -> SendReport {
        let payload = message.payload();
        let mut report = SendReport::default();
        // DECISION: devices are pushed one after another; a user has a
        // handful of devices and four events, so no fan-out machinery.
        for sub in subscriptions {
            let result = self.push_one(&payload, sub).await;
            tally(&mut report, &sub.endpoint, result);
        }
        report
    }

    /// Sign, encrypt and send `payload` to one device.
    async fn push_one(
        &self,
        payload: &str,
        sub: &Subscription,
    ) -> Result<(), web_push::WebPushError> {
        use web_push::WebPushClient;
        let info = web_push::SubscriptionInfo::new(
            sub.endpoint.as_str(),
            sub.p256dh.as_str(),
            sub.auth.as_str(),
        );
        let mut signature =
            web_push::VapidSignatureBuilder::from_base64(&self.keys.private_b64, &info)?;
        signature.add_claim("sub", self.subject.as_str());
        let mut builder = web_push::WebPushMessageBuilder::new(&info);
        builder.set_payload(web_push::ContentEncoding::Aes128Gcm, payload.as_bytes());
        builder.set_vapid_signature(signature.build()?);
        self.client.send(builder.build()?).await
    }
}

/// Count one device's outcome into `report`, logging every failure.
fn tally(report: &mut SendReport, endpoint: &str, result: Result<(), web_push::WebPushError>) {
    use web_push::WebPushError;
    match result {
        Ok(()) => report.delivered += 1,
        Err(e @ (WebPushError::EndpointNotValid(_) | WebPushError::EndpointNotFound(_))) => {
            eprintln!(
                "notify: push endpoint at {} is gone: {}",
                origin(endpoint),
                e.short_description()
            );
            report.gone.push(endpoint.to_owned());
        }
        Err(e) => {
            eprintln!(
                "notify: push to {} failed: {}",
                origin(endpoint),
                e.short_description()
            );
            report.failed += 1;
        }
    }
}

/// `scheme://host` of an endpoint, for logs.
// DECISION: logs never show the endpoint path: it is the device's
// bearer capability at the push service.
fn origin(endpoint: &str) -> String {
    endpoint.splitn(4, '/').take(3).collect::<Vec<_>>().join("/")
}

/// Why a VAPID key could not be generated, loaded or saved.
#[derive(Debug)]
pub enum NotifyError {
    /// Reading or writing the key file failed.
    Io(std::io::Error),
    /// The key bytes are not a valid P-256 private key.
    Key(String),
}

/// Stands in for a secret in `Debug` output.
const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("endpoint", &self.endpoint)
            .field("p256dh", &REDACTED)
            .field("auth", &REDACTED)
            .finish()
    }
}

impl std::fmt::Debug for VapidKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKeys")
            .field("public", &self.public_b64)
            .field("private", &REDACTED)
            .finish()
    }
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("keys", &self.keys)
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::Io(e) => write!(f, "VAPID key file: {e}"),
            NotifyError::Key(why) => write!(f, "invalid VAPID key: {why}"),
        }
    }
}

impl std::error::Error for NotifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            NotifyError::Io(e) => Some(e),
            NotifyError::Key(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        messages, tally, Message, MessageKind, Notifier, NotifyError, SendReport, Subscription,
        VapidKeys,
    };
    use crate::config::Config;
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
            // The push payload carries exactly that text and a URL.
            let payload: serde_json::Value =
                serde_json::from_str(&msg(ALICE, kind).payload()).unwrap();
            assert_eq!(
                payload,
                serde_json::json!({"title": text.title, "body": text.body, "url": "/"})
            );
        }
    }

    // notify-02
    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("atomanja-notify-{}-{name}", std::process::id()))
    }

    fn config() -> Config {
        let get = |k: &str| {
            let v = match k {
                "ATOMANJA_DB" => "/data/ledger.db",
                "ATOMANJA_DEV_DB" => "/data/dev.db",
                "ATOMANJA_VIKUNJA_URL" => "https://vikunja.example",
                "ATOMANJA_VIKUNJA_TOKEN" => "tok",
                "ATOMANJA_BASE_URL" => "https://atomanja.example",
                "ATOMANJA_SMTP_HOST" => "smtp.example",
                "ATOMANJA_SMTP_USER" => "mailer",
                "ATOMANJA_SMTP_PASSWORD" => "pw",
                "ATOMANJA_SMTP_FROM" => "noreply@atomanja.example",
                "ATOMANJA_VAPID_KEY" => "/data/vapid.key",
                _ => return None,
            };
            Some(v.to_owned())
        };
        Config::from_source(get).expect("test config")
    }

    #[test]
    fn app_33_vapid_key_roundtrip() {
        let keys = VapidKeys::generate().unwrap();
        let public = keys.public_key();
        // Uncompressed P-256 point: 65 bytes, 0x04 first, 87 base64url chars.
        assert_eq!(public.len(), 87, "{public}");
        assert!(public.starts_with('B'), "{public}");
        assert_ne!(VapidKeys::generate().unwrap(), keys);

        let path = temp_path("roundtrip");
        keys.save(&path).unwrap();
        let loaded = VapidKeys::load(&path).unwrap();
        assert_eq!(loaded, keys);
        assert_eq!(loaded.public_key(), public);

        std::fs::write(&path, "not a key\n").unwrap();
        assert!(matches!(VapidKeys::load(&path), Err(NotifyError::Key(_))));
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(VapidKeys::load(&path), Err(NotifyError::Io(_))));
    }

    #[test]
    fn notify_send_failure_is_logged_not_returned() {
        let keys = VapidKeys::generate().unwrap();
        // A valid browser key: any uncompressed P-256 point will do.
        let p256dh = VapidKeys::generate().unwrap().public_key();
        let auth = "AAAAAAAAAAAAAAAAAAAAAA".to_owned();
        let notifier = Notifier::new(keys, &config());
        let subs = [
            // Bad browser key: fails before any network I/O.
            Subscription {
                endpoint: "https://push.invalid/a".into(),
                p256dh: "!!".into(),
                auth: auth.clone(),
            },
            // Unparsable endpoint: fails before any network I/O.
            Subscription { endpoint: "not a url".into(), p256dh, auth },
        ];
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let message = msg(ALICE, MessageKind::Claimable);
        let report = rt.block_on(notifier.send(&message, &subs));
        assert_eq!(report, SendReport { delivered: 0, failed: 2, gone: vec![] });
        assert_eq!(rt.block_on(notifier.send(&message, &[])), SendReport::default());

        // How push-service answers are counted.
        // `ErrorInfo` is not re-exported by web-push; build it by inference.
        let info = |code: u16| {
            serde_json::from_value(serde_json::json!({
                "code": code, "errno": 0, "error": "", "message": ""
            }))
            .unwrap()
        };
        let mut r = SendReport::default();
        tally(&mut r, "https://p/ok", Ok(()));
        tally(&mut r, "https://p/410", Err(web_push::WebPushError::EndpointNotValid(info(410))));
        tally(&mut r, "https://p/404", Err(web_push::WebPushError::EndpointNotFound(info(404))));
        tally(&mut r, "https://p/500", Err(web_push::WebPushError::Unspecified));
        assert_eq!(
            r,
            SendReport {
                delivered: 1,
                failed: 1,
                gone: vec!["https://p/410".into(), "https://p/404".into()],
            }
        );
    }

    #[test]
    fn notify_debug_hides_secrets() {
        let sub = Subscription {
            endpoint: "https://push.example/ep".into(),
            p256dh: "P256DH-SECRET".into(),
            auth: "AUTH-SECRET".into(),
        };
        let s = format!("{sub:?}");
        assert!(s.contains("https://push.example/ep"), "{s}");
        assert!(!s.contains("P256DH-SECRET") && !s.contains("AUTH-SECRET"), "{s}");

        let keys = VapidKeys::generate().unwrap();
        let private = keys.private_b64.clone();
        let s = format!("{keys:?} {keys:#?}");
        assert!(!s.contains(&private), "{s}");
        let s = format!("{:?}", Notifier::new(keys, &config()));
        assert!(!s.contains(&private), "{s}");
        assert!(s.contains("https://atomanja.example"), "{s}");

        let err = NotifyError::Key("bad length".into());
        assert!(err.to_string().contains("bad length"));
        assert!(std::error::Error::source(&err).is_none());
        let io = NotifyError::Io(std::io::Error::other("disk"));
        assert!(std::error::Error::source(&io).is_some());
    }

    // notify-03
    #[test] fn app_38_notification_default_french() { todo!() }
    #[test] fn app_39_notification_uses_recipient_language() { todo!() }
}
