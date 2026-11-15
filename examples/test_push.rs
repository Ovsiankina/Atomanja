//! One-off: list push devices per account in both databases, and push a
//! test notification to every account matching an argument. Each
//! argument is an email or a role name (`worker`, `witness`, `admin`).
use atomanja::config::Config;
use atomanja::ledger::{Centimes, DebtId, TaskId};
use atomanja::notify::{Message, MessageKind, Notifier, Subscription, VapidKeys};
use atomanja::store::{DevDb, Kind, LedgerDb, Store};

async fn run<K: Kind>(name: &str, store: Store<K>, notifier: &Notifier, targets: &[String]) {
    for acc in store.accounts().expect("accounts") {
        let subs: Vec<Subscription> = store.subscriptions(acc.id).expect("subs").into_iter()
            .map(|s| Subscription { endpoint: s.endpoint, p256dh: s.p256dh, auth: s.auth }).collect();
        let role = format!("{:?}", acc.role).to_lowercase();
        println!("[{name}] {} ({role}): {} device(s)", acc.email, subs.len());
        let wanted = targets.iter().any(|t| *t == acc.email || t.to_lowercase() == role);
        if subs.is_empty() || !wanted { continue; }
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
    let targets: Vec<String> = std::env::args().skip(1).collect();
    run("ledger", Store::<LedgerDb>::open(&config.ledger_db).expect("db"), &notifier, &targets).await;
    run("dev", Store::<DevDb>::open(&config.dev_db).expect("dev db"), &notifier, &targets).await;
}
