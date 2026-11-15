//! Atomanja: turns missed Vikunja deadlines into debts owed to a witness.
//!
//! Module order (dependencies first) is documented in
//! `docs/plans/2026-11-15-hld.md` section 3.

pub mod config;
pub mod ledger;
pub mod auth;
pub mod vikunja;
pub mod notify;
pub mod mail;
pub mod store;
pub mod sync;
pub mod devdata;
pub mod web;
