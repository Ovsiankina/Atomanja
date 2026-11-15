//! The settings screen: accounts and invite link (APP-26), amount
//! proposal (APP-16), language (APP-39), notifications (APP-33).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use maud::{html, Markup};

use super::handlers::{page, require};
use super::i18n::{msgs, Msgs};
use super::pages::{self, Field, Variant};
use super::session::{internal, lock};
use super::AppState;
use crate::auth::{self, Account, Action, Database, Role};
use crate::store::{Kind, Store, StoreError};

/// A freshly created invite link and what happened to its email.
pub(super) struct InviteOutcome {
    pub link: String,
    /// `(address, sent)` when the worker entered an address.
    pub mailed: Option<(String, bool)>,
}

/// `GET /settings`.
pub(super) async fn settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let actor = match require(&state, &headers) {
        Ok(a) => a,
        Err(r) => return r,
    };
    render(&state, &actor.account, None)
}

/// The full settings page for `account`, from its role's file (DEV-3).
pub(super) fn render(state: &AppState, account: &Account, invite: Option<&InviteOutcome>) -> Response {
    let m = msgs(account.lang);
    let body = match account.role.database() {
        Database::Ledger => body(&*lock(&state.ledger), account, invite),
        Database::Dev => body(&*lock(&state.dev), account, invite),
    };
    match body {
        Ok(b) => page(m, m.nav_settings, "settings", &b),
        Err(e) => internal(&e),
    }
}

/// The settings sections over `store`.
fn body<K: Kind>(
    store: &Store<K>,
    account: &Account,
    invite: Option<&InviteOutcome>,
) -> Result<Markup, StoreError> {
    let m = msgs(account.lang);
    let accounts = store.accounts()?;
    Ok(html! {
        div.page__stack data-view="settings" {
            @if account.role == Role::Admin {
                div.page__pinned { (pages::banner(true, m.fake_label, m.fake_text)) }
            }
            (pages::section(1, m.accounts, &account_panel(m, &accounts, account, invite)))
        }
    })
}

/// The `AccountPanel`: the one worker, the witnesses, and for the worker
/// the invite block. Its `id` is the htmx target of `POST /invite`.
pub(super) fn account_panel(
    m: &Msgs,
    accounts: &[Account],
    me: &Account,
    invite: Option<&InviteOutcome>,
) -> Markup {
    html! {
        div.at-account id="accounts" {
            header.at-account__head {
                span.at-label { (m.accounts) }
                span.at-mono { (accounts.len()) }
            }
            ul.at-people {
                @for a in accounts {
                    li.at-person {
                        span.at-person__name {
                            (a.email)
                            @if a.id == me.id { span.at-index { " ·" } }
                        }
                        span.at-label { (match a.role {
                            Role::Worker => m.role_worker,
                            Role::Witness => m.role_witness,
                            Role::Admin => m.role_admin,
                        }) }
                        span.at-muted { (match a.role {
                            Role::Worker => m.owes_when_late,
                            Role::Witness => m.active,
                            Role::Admin => m.sees_fake,
                        }) }
                    }
                }
            }
            @if auth::permits(me, Action::CreateInvite) {
                div.at-account__invite {
                    @if let Some(inv) = invite {
                        (Field { readonly: true, required: false, value: &inv.link, hint: Some(m.invite_hint),
                                 ..Field::new(m.invite_link, "link", "text", "off") }.render())
                        @if let Some((to, sent)) = &inv.mailed {
                            @if *sent { p.at-muted role="status" { (m.mail_sent(to)) } }
                            @else { p.row__error role="alert" { (m.mail_failed) } }
                        }
                    }
                    form.page__form method="post" action="/invite" hx-post="/invite" hx-target="#accounts" hx-swap="outerHTML" {
                        (Field { required: false, hint: Some(m.invite_email_hint),
                                 ..Field::new(m.invite_email, "email", "email", "off") }.render())
                        (pages::button(if invite.is_some() { m.invite_another } else { m.invite_witness },
                                       Variant::Outline, false, false, Some("→")))
                    }
                }
            }
        }
    }
}
