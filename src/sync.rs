//! The poll loop: fetch from Vikunja, apply to the ledger, persist,
//! notify. Runs once at startup, then every poll interval.
//!
//! Uses: `config`, `vikunja`, `ledger`, `store`, `notify`.
//! SRS: APP-2, APP-9, DB-3, SRS 5.1.
