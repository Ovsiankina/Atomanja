//! The message catalogue: one per language, compiled into the binary
//! (APP-38..APP-40). Every user-facing text of the pages comes from here.

// DECISION: a plain struct of `&'static str` per language plus a few
// formatting methods, chosen with a `match` on `Lang`. No i18n crate (no
// new dependency), no runtime files, and a missing string is a compile
// error because both catalogues fill the same struct.

use crate::config::Lang;
use crate::ledger::{Centimes, Timestamp};

/// Every fixed text of the interface, in one language.
pub(super) struct Msgs {
    pub lang: Lang,
    pub sign_in_label: &'static str,
    pub email: &'static str,
    pub password: &'static str,
    pub sign_in: &'static str,
    pub forgot: &'static str,
    pub login_error: &'static str,
    pub sign_out: &'static str,
    pub nav_tasks: &'static str,
    pub nav_settings: &'static str,
    pub owed: &'static str,
    pub sent: &'static str,
    pub confirmed: &'static str,
    pub st_upcoming: &'static str,
    pub st_met: &'static str,
    pub st_late: &'static str,
    pub st_sent: &'static str,
    pub st_received: &'static str,
    pub unclaimed: &'static str,
    pub you: &'static str,
    pub sr_yes: &'static str,
    pub sr_not_yet: &'static str,
    pub claim: &'static str,
    pub mark_sent: &'static str,
    pub mark_received: &'static str,
    pub ev_removed: &'static str,
    pub ev_deleted: &'static str,
    pub ev_restored: &'static str,
    pub tasks: &'static str,
    pub no_tasks: &'static str,
    pub fake_label: &'static str,
    pub fake_text: &'static str,
    pub info_label: &'static str,
    pub info_text: &'static str,
    pub err_already_claimed: &'static str,
    pub err_forbidden: &'static str,
    pub err_state: &'static str,
    pub err_unknown_debt: &'static str,
    pub err_server: &'static str,
    pub err_not_found: &'static str,
    pub accounts: &'static str,
    pub role_worker: &'static str,
    pub role_witness: &'static str,
    pub role_admin: &'static str,
    pub owes_when_late: &'static str,
    pub active: &'static str,
    pub sees_fake: &'static str,
    pub invite_witness: &'static str,
    pub invite_another: &'static str,
    pub invite_email: &'static str,
    pub invite_email_hint: &'static str,
    pub invite_link: &'static str,
    pub invite_hint: &'static str,
    pub mail_failed: &'static str,
    pub amount_per_task: &'static str,
    pub propose_field: &'static str,
    pub propose_hint: &'static str,
    pub propose: &'static str,
    pub proposed_change: &'static str,
    pub vote_agreed: &'static str,
    pub vote_pending: &'static str,
    pub agree: &'static str,
    pub err_amount: &'static str,
    pub language: &'static str,
    pub other_lang_name: &'static str,
    pub notif_label: &'static str,
    pub notif_steps: [&'static str; 3],
    pub notif_enable: &'static str,
    pub notif_unlock: &'static str,
    pub invitation: &'static str,
    pub invite_intro: &'static str,
    pub new_password: &'static str,
    pub repeat_password: &'static str,
    pub accept_invite: &'static str,
    pub err_mismatch: &'static str,
    pub err_too_short: &'static str,
    pub err_email_taken: &'static str,
    pub err_email_empty: &'static str,
    pub reset_label: &'static str,
    pub reset_title: &'static str,
    pub reset_intro: &'static str,
    pub send_link: &'static str,
    pub reset_sent: &'static str,
    pub set_password: &'static str,
    pub reset_done: &'static str,
    pub link_label: &'static str,
    pub link_expired: &'static str,
    pub link_used: &'static str,
    pub link_unknown: &'static str,
    pub back_to_login: &'static str,
    pub a_worker: &'static str,
    months: [&'static str; 12],
    decimal: char,
}

/// English.
const EN: Msgs = Msgs {
    lang: Lang::En,
    sign_in_label: "Sign in",
    email: "Email",
    password: "Password",
    sign_in: "Sign in",
    forgot: "Forgot your password",
    login_error: "Wrong email or password.",
    sign_out: "Sign out",
    nav_tasks: "Tasks",
    nav_settings: "Settings",
    owed: "Owed",
    sent: "Sent",
    confirmed: "Confirmed",
    st_upcoming: "Upcoming",
    st_met: "Met",
    st_late: "Late",
    st_sent: "Sent",
    st_received: "Received",
    unclaimed: "Unclaimed",
    you: "you",
    sr_yes: ": yes",
    sr_not_yet: ": not yet",
    claim: "Claim",
    mark_sent: "Mark sent",
    mark_received: "Mark received",
    ev_removed: "Deadline removed in Vikunja. It still counts.",
    ev_deleted: "Task deleted in Vikunja. The deadline still counts.",
    ev_restored: "Deadline set again in Vikunja. It still counts.",
    tasks: "Tasks",
    no_tasks: "No tracked task yet.",
    fake_label: "Fake data",
    fake_text: "Admin view. Nothing on this screen is a real task or a real debt.",
    info_label: "No money handling",
    info_text: "Atomanja only counts. Payment happens outside it.",
    err_already_claimed: "Already claimed by another witness.",
    err_forbidden: "Your account cannot do this.",
    err_state: "Not possible in this state.",
    err_unknown_debt: "No such debt.",
    err_server: "Something went wrong. Try again.",
    err_not_found: "No such page.",
    accounts: "Accounts",
    role_worker: "Worker",
    role_witness: "Witness",
    role_admin: "Admin witness",
    owes_when_late: "Owes when late",
    active: "Active",
    sees_fake: "Sees fake data only",
    invite_witness: "Invite a witness",
    invite_another: "Create another link",
    invite_email: "Their email",
    invite_email_hint: "Optional. Atomanja also emails the link there.",
    invite_link: "Invite link",
    invite_hint: "Send it yourself. It works once, for 7 days. The witness sets their own password.",
    mail_failed: "Email not sent. Copy the link.",
    amount_per_task: "Amount per late task",
    propose_field: "Propose a new amount",
    propose_hint: "It changes only when every witness agrees.",
    propose: "Propose",
    proposed_change: "Proposed amount change",
    vote_agreed: "Agreed",
    vote_pending: "Pending",
    agree: "Agree",
    err_amount: "Enter a whole number of CHF, above zero and different from the current amount.",
    language: "Language",
    other_lang_name: "Français",
    notif_label: "Notifications on iPhone",
    notif_steps: [
        "Open Atomanja in Safari.",
        "Tap Share, then \"Add to Home Screen\".",
        "Open Atomanja from the Home Screen.",
    ],
    notif_enable: "Enable notifications",
    notif_unlock: "The button works once Atomanja runs from the Home Screen.",
    invitation: "Invitation",
    invite_intro: "Set a password to accept. You will see task numbers and deadlines, never titles.",
    new_password: "New password",
    repeat_password: "Repeat password",
    accept_invite: "Accept invite",
    err_mismatch: "The two passwords differ.",
    err_too_short: "The password needs at least 8 characters.",
    err_email_taken: "This email already has an account.",
    err_email_empty: "Enter your email.",
    reset_label: "Password reset",
    reset_title: "Forgot your password",
    reset_intro: "Enter your email. If it has an account, Atomanja sends it a link that works once, for 1 hour.",
    send_link: "Send the link",
    reset_sent: "If this email has an account, a link is on its way. It works once, for 1 hour.",
    set_password: "Set password",
    reset_done: "Password changed. Sign in with the new one.",
    link_label: "Link",
    link_expired: "This link has expired. Ask for a new one.",
    link_used: "This link was already used. Ask for a new one.",
    link_unknown: "This link is not valid.",
    back_to_login: "Back to sign in",
    a_worker: "The worker",
    months: [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    decimal: '.',
};

/// French, the default (APP-38).
const FR: Msgs = Msgs {
    lang: Lang::Fr,
    sign_in_label: "Connexion",
    email: "Adresse e-mail",
    password: "Mot de passe",
    sign_in: "Se connecter",
    forgot: "Mot de passe oublié",
    login_error: "Adresse ou mot de passe incorrect.",
    sign_out: "Se déconnecter",
    nav_tasks: "Tâches",
    nav_settings: "Réglages",
    owed: "Dû",
    sent: "Envoyé",
    confirmed: "Confirmé",
    st_upcoming: "À venir",
    st_met: "Tenu",
    st_late: "En retard",
    st_sent: "Envoyé",
    st_received: "Reçu",
    unclaimed: "Non réclamée",
    you: "vous",
    sr_yes: " : oui",
    sr_not_yet: " : pas encore",
    claim: "Réclamer",
    mark_sent: "Marquer envoyé",
    mark_received: "Marquer reçu",
    ev_removed: "Échéance supprimée dans Vikunja. Elle compte toujours.",
    ev_deleted: "Tâche supprimée dans Vikunja. L'échéance compte toujours.",
    ev_restored: "Échéance remise dans Vikunja. Elle compte toujours.",
    tasks: "Tâches",
    no_tasks: "Aucune tâche suivie pour l'instant.",
    fake_label: "Données fictives",
    fake_text: "Vue admin. Rien sur cet écran n'est une vraie tâche ni une vraie dette.",
    info_label: "Pas d'argent ici",
    info_text: "Atomanja ne fait que compter. Le paiement se fait ailleurs.",
    err_already_claimed: "Déjà réclamée par un autre témoin.",
    err_forbidden: "Votre compte ne peut pas faire cela.",
    err_state: "Impossible dans cet état.",
    err_unknown_debt: "Dette inconnue.",
    err_server: "Une erreur est survenue. Réessayez.",
    err_not_found: "Page introuvable.",
    accounts: "Comptes",
    role_worker: "Travailleur",
    role_witness: "Témoin",
    role_admin: "Témoin admin",
    owes_when_late: "Doit en cas de retard",
    active: "Actif",
    sees_fake: "Ne voit que des données fictives",
    invite_witness: "Inviter un témoin",
    invite_another: "Créer un autre lien",
    invite_email: "Son adresse e-mail",
    invite_email_hint: "Facultatif. Atomanja y envoie aussi le lien.",
    invite_link: "Lien d'invitation",
    invite_hint: "Envoyez-le vous-même. Il sert une fois, pendant 7 jours. Le témoin choisit son mot de passe.",
    mail_failed: "E-mail non envoyé. Copiez le lien.",
    amount_per_task: "Montant par tâche en retard",
    propose_field: "Proposer un nouveau montant",
    propose_hint: "Il ne change que si tous les témoins sont d'accord.",
    propose: "Proposer",
    proposed_change: "Changement de montant proposé",
    vote_agreed: "D'accord",
    vote_pending: "En attente",
    agree: "Accepter",
    err_amount: "Indiquez un nombre entier de CHF, plus grand que zéro et différent du montant actuel.",
    language: "Langue",
    other_lang_name: "English",
    notif_label: "Notifications sur iPhone",
    notif_steps: [
        "Ouvrez Atomanja dans Safari.",
        "Touchez Partager, puis « Sur l'écran d'accueil ».",
        "Ouvrez Atomanja depuis l'écran d'accueil.",
    ],
    notif_enable: "Activer les notifications",
    notif_unlock: "Le bouton fonctionne quand Atomanja tourne depuis l'écran d'accueil.",
    invitation: "Invitation",
    invite_intro: "Choisissez un mot de passe pour accepter. Vous verrez les numéros de tâches et les échéances, jamais les titres.",
    new_password: "Nouveau mot de passe",
    repeat_password: "Répétez le mot de passe",
    accept_invite: "Accepter l'invitation",
    err_mismatch: "Les deux mots de passe diffèrent.",
    err_too_short: "Le mot de passe doit faire au moins 8 caractères.",
    err_email_taken: "Cette adresse a déjà un compte.",
    err_email_empty: "Indiquez votre adresse e-mail.",
    reset_label: "Réinitialisation",
    reset_title: "Mot de passe oublié",
    reset_intro: "Indiquez votre adresse. Si elle a un compte, Atomanja lui envoie un lien qui sert une fois, pendant 1 heure.",
    send_link: "Envoyer le lien",
    reset_sent: "Si cette adresse a un compte, un lien est en route. Il sert une fois, pendant 1 heure.",
    set_password: "Choisir ce mot de passe",
    reset_done: "Mot de passe changé. Connectez-vous avec le nouveau.",
    link_label: "Lien",
    link_expired: "Ce lien a expiré. Demandez-en un nouveau.",
    link_used: "Ce lien a déjà servi. Demandez-en un nouveau.",
    link_unknown: "Ce lien n'est pas valide.",
    back_to_login: "Retour à la connexion",
    a_worker: "Le travailleur",
    months: [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    decimal: ',',
};

/// The catalogue for `lang`.
pub(super) fn msgs(lang: Lang) -> &'static Msgs {
    match lang {
        Lang::Fr => &FR,
        Lang::En => &EN,
    }
}

impl Msgs {
    /// `Task 47` / `Tâche 47`: the only name a witness ever sees (APP-32).
    pub fn task(&self, n: i64) -> String {
        match self.lang {
            Lang::Fr => format!("Tâche {n}"),
            Lang::En => format!("Task {n}"),
        }
    }

    /// `Claimed by …`.
    pub fn claimed_by(&self, who: &str) -> String {
        match self.lang {
            Lang::Fr => format!("Réclamée par {who}"),
            Lang::En => format!("Claimed by {who}"),
        }
    }

    /// A due date moved later (or set again later) in Vikunja.
    pub fn ev_moved(&self, to: Timestamp, recorded: Timestamp) -> String {
        let (to, from) = (self.date(to), self.date(recorded));
        match self.lang {
            Lang::Fr => format!("Échéance déplacée au {to} dans Vikunja. Le {from} compte toujours."),
            Lang::En => format!("Deadline moved to {to} in Vikunja. {from} still counts."),
        }
    }

    /// A due date moved earlier: the recorded deadline follows (APP-7).
    pub fn ev_earlier(&self, to: Timestamp) -> String {
        let to = self.date(to);
        match self.lang {
            Lang::Fr => format!("Échéance avancée au {to} dans Vikunja. Le {to} compte."),
            Lang::En => format!("Deadline moved earlier to {to} in Vikunja. {to} counts."),
        }
    }

    /// The invite sentence on the accept page.
    pub fn invite_title(&self, inviter: &str) -> String {
        match self.lang {
            Lang::Fr => format!("{inviter} vous demande d'être témoin de ses échéances."),
            Lang::En => format!("{inviter} asks you to witness their deadlines."),
        }
    }

    /// `Also emailed to …`.
    pub fn mail_sent(&self, to: &str) -> String {
        match self.lang {
            Lang::Fr => format!("Aussi envoyé par e-mail à {to}."),
            Lang::En => format!("Also emailed to {to}."),
        }
    }

    /// `1 of 2 witnesses agreed …`.
    pub fn agreed_count(&self, agreed: usize, total: usize) -> String {
        match self.lang {
            Lang::Fr => format!(
                "{agreed} témoin(s) sur {total} d'accord. Il ne change que si tous sont d'accord."
            ),
            Lang::En => format!(
                "{agreed} of {total} witnesses agreed. It changes only when all agree."
            ),
        }
    }

    /// `14 Nov` / `14 nov.`, UTC (SRS 5.1).
    pub fn date(&self, at: Timestamp) -> String {
        match time::OffsetDateTime::from_unix_timestamp(at.0) {
            Ok(t) => format!("{} {}", t.day(), self.months[usize::from(u8::from(t.month())) - 1]),
            // DECISION: an out-of-range stored time prints as `?`, never panics.
            Err(_) => "?".to_owned(),
        }
    }

    /// The number part of an amount: `10`, or `12.50` / `12,50`.
    pub fn number(&self, amount: Centimes) -> String {
        let (whole, cents) = (amount.0 / 100, amount.0 % 100);
        if cents == 0 {
            whole.to_string()
        } else {
            format!("{whole}{}{cents:02}", self.decimal)
        }
    }

    /// `10 CHF`: number then currency.
    pub fn amount(&self, amount: Centimes) -> String {
        format!("{} CHF", self.number(amount))
    }
}
