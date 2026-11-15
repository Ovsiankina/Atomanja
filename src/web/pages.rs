//! Page frame and design-system components, rendered server-side with
//! maud. The markup and class names follow `design-system/components/
//! bundle.js` (the React reference) so `bundle.css` styles them as is.

use maud::{html, Markup, PreEscaped, DOCTYPE};

use super::i18n::Msgs;
use crate::ledger::Totals;

/// htmx settings: swap 409 answers too (a rejected action re-renders its
/// row with the reason), other 4xx/5xx are errors and not swapped.
const HTMX_CONFIG: &str = r#"{"responseHandling":[{"code":"204","swap":false},{"code":"[23]..","swap":true},{"code":"409","swap":true},{"code":"[45]..","swap":false,"error":true}]}"#;

/// The signed-in part of the frame: nav and sign out.
pub(super) struct Nav {
    /// Which nav entry is current: `"tasks"` or `"settings"`.
    pub current: &'static str,
}

/// The whole HTML document.
pub(super) fn layout(m: &Msgs, title: &str, nav: Option<&Nav>, content: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(m.lang.code()) data-theme="dark" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { (title) " · Atomanja" }
                meta name="theme-color" content="#1f1f1f";
                meta name="htmx-config" content=(HTMX_CONFIG);
                link rel="manifest" href="/manifest.webmanifest";
                link rel="icon" href="/assets/icon.svg" type="image/svg+xml";
                link rel="apple-touch-icon" href="/assets/icon.svg";
                link rel="preconnect" href="https://fonts.googleapis.com";
                link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Hanken+Grotesk:wght@300..700&family=Geist+Mono:wght@400..600&display=swap";
                link rel="stylesheet" href="/assets/tokens.css";
                link rel="stylesheet" href="/assets/bundle.css";
                link rel="stylesheet" href="/assets/app.css";
                script src="/assets/htmx.min.js" defer {}
                script src="/assets/app.js" defer {}
            }
            body.at-root {
                @if let Some(nav) = nav {
                    div.page {
                        header.page__head {
                            a.page__mark href="/" { "Atomanja" }
                            nav.page__nav {
                                a.page__link.is-on[nav.current == "tasks"] href="/" { (m.nav_tasks) }
                                a.page__link.is-on[nav.current == "settings"] href="/settings" { (m.nav_settings) }
                                form method="post" action="/logout" {
                                    (button(m.sign_out, Variant::Ghost, false, false, None))
                                }
                            }
                        }
                        main.page__main { (content) }
                    }
                } @else {
                    main.page.page--auth { (content) }
                }
            }
        }
    }
}

/// Button styles (`Button` README).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Variant {
    Solid,
    Outline,
    Ghost,
}

/// A submit button: `at-btn`, verb-first label, optional mono arrow.
pub(super) fn button(
    label: &str,
    variant: Variant,
    small: bool,
    block: bool,
    arrow: Option<&str>,
) -> Markup {
    let v = match variant {
        Variant::Solid => "at-btn--solid",
        Variant::Outline => "at-btn--outline",
        Variant::Ghost => "at-btn--ghost",
    };
    html! {
        button type="submit" class={ "at-btn " (v) @if small { " at-btn--sm" } @if block { " at-btn--block" } } {
            span { (label) }
            @if let Some(a) = arrow { span.at-btn__arrow aria-hidden="true" { (a) } }
        }
    }
}

/// One text input (`Field`).
pub(super) struct Field<'a> {
    pub label: &'a str,
    pub name: &'a str,
    pub kind: &'a str,
    pub autocomplete: &'a str,
    pub value: &'a str,
    pub hint: Option<&'a str>,
    pub error: Option<&'a str>,
    pub required: bool,
    pub readonly: bool,
}

impl<'a> Field<'a> {
    /// A required field of `kind` with no value, hint or error.
    pub fn new(label: &'a str, name: &'a str, kind: &'a str, autocomplete: &'a str) -> Self {
        Field {
            label,
            name,
            kind,
            autocomplete,
            value: "",
            hint: None,
            error: None,
            required: true,
            readonly: false,
        }
    }

    /// Render it.
    pub fn render(&self) -> Markup {
        let id = format!("at-f-{}", self.name);
        let note = self.error.or(self.hint);
        html! {
            div class={ "at-field" @if self.error.is_some() { " at-field--error" } } {
                label.at-label for=(id) { (self.label) }
                input.at-input id=(id) name=(self.name) type=(self.kind)
                    autocomplete=(self.autocomplete) value=(self.value)
                    required[self.required] readonly[self.readonly]
                    onfocus=[self.readonly.then_some("this.select()")];
                @if let Some(n) = note { p.at-field__hint { (n) } }
            }
        }
    }
}

/// The task statuses the views print. `Met` is shown with the neutral
/// tag: the design system has four debt states, the SRS also needs met
/// (APP-31, APP-32).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Status {
    Upcoming,
    Met,
    Late,
    Sent,
    Received,
}

impl Status {
    /// CSS suffix shared by `at-row--*` and `at-tag--*`.
    pub fn class(self) -> &'static str {
        match self {
            Status::Upcoming => "upcoming",
            Status::Met => "neutral",
            Status::Late => "late",
            Status::Sent => "sent",
            Status::Received => "received",
        }
    }

    /// The status word (mandatory next to its colour).
    pub fn word(self, m: &Msgs) -> &'static str {
        match self {
            Status::Upcoming => m.st_upcoming,
            Status::Met => m.st_met,
            Status::Late => m.st_late,
            Status::Sent => m.st_sent,
            Status::Received => m.st_received,
        }
    }
}

/// A status word in a hairline box (`Tag`).
pub(super) fn tag(class: &str, word: &str) -> Markup {
    html! {
        span class={ "at-tag at-tag--" (class) } {
            i.at-tag__mark aria-hidden="true" {}
            (word)
        }
    }
}

/// A full-width strip (`Banner`); `fake` gives the dashed admin banner.
pub(super) fn banner(fake: bool, label: &str, text: &str) -> Markup {
    html! {
        div class={ "at-banner " @if fake { "at-banner--fake" } @else { "at-banner--info" } } role="status" {
            span.at-banner__label { (label) }
            span { (text) }
        }
    }
}

/// One compact mark (`check` in the task views).
pub(super) fn check(m: &Msgs, name: &str, on: bool) -> Markup {
    html! {
        span class={ "at-check" @if on { " is-on" } } {
            i.at-check__box aria-hidden="true" { @if on { "✓" } }
            (name)
            span.at-sr { @if on { (m.sr_yes) } @else { (m.sr_not_yet) } }
        }
    }
}

/// Owed, sent, confirmed (`Totals`). `oob` marks it for an htmx
/// out-of-band swap so an action fragment refreshes the totals too.
pub(super) fn totals(m: &Msgs, t: &Totals, oob: bool) -> Markup {
    let items = [
        (m.owed, t.owed, "late"),
        (m.sent, t.sent, "sent"),
        (m.confirmed, t.confirmed, "received"),
    ];
    html! {
        dl.at-totals id="totals" hx-swap-oob=[oob.then_some("true")] {
            @for (i, (name, amount, tone)) in items.iter().enumerate() {
                div class={ "at-total" @if amount.0 > 0 { " at-total--" (tone) } } {
                    dt.at-label { span.at-index { "/0." (i + 1) } " " (name) }
                    dd.at-total__n { (m.number(*amount)) span.at-unit { "CHF" } }
                }
            }
        }
    }
}

/// A numbered section: `line-strong` rule, `/0.n` index and label.
pub(super) fn section(index: usize, label: &str, body: &Markup) -> Markup {
    html! {
        section.page__section {
            p.at-label.page__section-head { span.at-index { "/0." (index) } " " (label) }
            (body)
        }
    }
}

/// The centred auth frame (`AuthCard`): label, title, intro, fields,
/// error above the one solid button.
pub(super) struct AuthCard<'a> {
    pub action: &'a str,
    pub label: &'a str,
    pub title: &'a str,
    pub big_title: bool,
    pub intro: Option<&'a str>,
    pub fields: Vec<Field<'a>>,
    pub error: Option<&'a str>,
    pub notice: Option<&'a str>,
    pub submit: &'a str,
    pub footer: Option<(&'a str, &'a str)>,
}

impl AuthCard<'_> {
    /// Render it.
    pub fn render(&self) -> Markup {
        html! {
            form method="post" action=(self.action)
                class={ "at-auth" @if !self.big_title { " at-auth--invite" } } {
                p.at-label { (self.label) }
                h1.at-auth__title { (self.title) }
                @if let Some(i) = self.intro { p.at-muted { (i) } }
                @if let Some(n) = self.notice { p.at-muted role="status" { (n) } }
                @for f in &self.fields { (f.render()) }
                @if let Some(e) = self.error { p.at-auth__error role="alert" { (e) } }
                (button(self.submit, Variant::Solid, false, true, Some("→")))
                @if let Some((href, text)) = self.footer {
                    a.page__link href=(href) { (text) }
                }
            }
        }
    }
}

/// A message-only auth frame: an expired link, a sent reset mail.
pub(super) fn notice_card(label: &str, title: &str, text: &str, link: (&str, &str)) -> Markup {
    html! {
        div.at-auth.at-auth--invite {
            p.at-label { (label) }
            h1.at-auth__title { (title) }
            p.at-muted role="alert" { (text) }
            a.page__link href=(link.0) { (link.1) }
        }
    }
}

/// Literal pre-escaped markup (static SVG glyphs etc.).
#[allow(dead_code)]
pub(super) fn raw(s: &'static str) -> PreEscaped<&'static str> {
    PreEscaped(s)
}
