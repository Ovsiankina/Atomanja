//! One-off: list push devices per account in both databases, and push a
//! test notification to the account whose email is given.
use atomanja::config::Config;
use atomanja::ledger::{Centimes, DebtId, TaskId};
use atomanja::notify::{Message, MessageKind, Notifier, Subscription, VapidKeys};
use atomanja::store::{DevDb, Kind, LedgerDb, Store};

async fn run<K: Kind>(name: &str, store: Store<K>, notifier: &Notifier, only: Option<&str>) {
    for acc in store.accounts().expect("accounts") {
        let subs: Vec<Subscription> = store.subscriptions(acc.id).expect("subs").into_iter()
            .map(|s| Subscription { endpoint: s.endpoint, p256dh: s.p256dh, auth: s.auth }).collect();
        println!("[{name}] {}: {} device(s)", acc.email, subs.len());
        if subs.is_empty() || only != Some(acc.email.as_str()) { continue; }
        let msg = Message { to: acc.id, lang: acc.lang, kind: MessageKind::ClaimedForYou,
            debt: DebtId(0), task: TaskId(0), amount: Centimes(100) };
        let r = notifier.send(&msg, &subs).await;
        println!("  -> delivered {} failed {} gone {}", r.delivered, r.failed, r.gone.len());
    }
}

#[tokio::main]
async fn main() {
    let config = Config::from_env().expect("config");
    let keys = VapidKeys::load(&config.vapid_key).expect("vapid key");
    let notifier = Notifier::new(keys, &config);
    let only = std::env::args().nth(1);
    run("ledger", Store::<LedgerDb>::open(&config.ledger_db).expect("db"), &notifier, only.as_deref()).await;
    run("dev", Store::<DevDb>::open(&config.dev_db).expect("dev db"), &notifier, only.as_deref()).await;
}
