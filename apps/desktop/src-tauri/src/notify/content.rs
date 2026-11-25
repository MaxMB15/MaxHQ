// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What a notification on Linux or Windows shows, worked out from the
//! app's notification and what the desktop can do. It's apart from the
//! sender so it can be tested on any system.

use maxhq_app::notify::Notification;

use super::NotificationPermission;

/// What this desktop's notifications can do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Abilities {
    /// Whether anything shows them. It's off when no notification server
    /// answered.
    pub shows: bool,
    /// Whether a click comes back to the app, which then shows the agent.
    pub clicks: bool,
    /// Whether the body is read as markup, so its text needs escaping.
    pub markup: bool,
    /// Whether a notification has a line for the subtitle. Linux's have
    /// only a summary and a body, so the subtitle goes in the body there.
    pub subtitle: bool,
}

impl Abilities {
    /// What a Linux notification server can do, from its answer to
    /// `GetCapabilities`.
    #[cfg(any(target_os = "linux", test))]
    pub fn server<S: AsRef<str>>(capabilities: &[S]) -> Self {
        let has = |name: &str| capabilities.iter().any(|each| each.as_ref() == name);
        Self {
            shows: true,
            clicks: has("actions"),
            markup: has("body-markup"),
            subtitle: false,
        }
    }

    /// What the settings page shows. There's nothing to ask for on these
    /// systems: notifications show whenever something is there to show
    /// them.
    pub fn permission(self) -> NotificationPermission {
        if self.shows {
            NotificationPermission::Allowed
        } else {
            NotificationPermission::Unavailable
        }
    }
}

/// What one notification shows.
#[derive(Debug, PartialEq, Eq)]
pub struct Content {
    /// The agent's title.
    pub summary: String,
    /// Where the agent runs, when the notification has a line for it.
    /// Linux's never do, so Linux doesn't read this.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub subtitle: Option<String>,
    /// What happened, and where the agent runs when there's no subtitle
    /// line.
    pub body: String,
    /// Whether it plays a sound.
    pub sound: bool,
    /// Whether it asks to hear about a click. A toast reports a click
    /// without asking, so Windows doesn't read this.
    #[cfg_attr(windows, allow(dead_code))]
    pub click: bool,
}

impl Content {
    /// What `notification` shows on a desktop that can do `abilities`.
    pub fn new(notification: &Notification, abilities: Abilities) -> Self {
        let text = |text: &str| {
            if abilities.markup {
                escape(text)
            } else {
                text.to_owned()
            }
        };
        let body = text(&notification.body);
        let (subtitle, body) = match &notification.subtitle {
            Some(subtitle) if abilities.subtitle => (Some(subtitle.clone()), body),
            // What happened comes first, since some desktops show only the
            // body's first line until you open the notification.
            Some(subtitle) => (None, format!("{body}\n{}", text(subtitle))),
            None => (None, body),
        };
        Self {
            summary: notification.title.clone(),
            subtitle,
            body,
            sound: notification.sound,
            click: abilities.clicks,
        }
    }
}

/// Escapes the characters the notification spec's body markup reads as
/// markup, so an agent's text shows as written.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for each in text.chars() {
        match each {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use maxhq_protocol::ids::AgentId;

    use super::*;

    fn notification(subtitle: Option<&str>) -> Notification {
        Notification {
            id: "attention".into(),
            agent: AgentId::derive("attention"),
            title: "Fix <the> merge".into(),
            subtitle: subtitle.map(Into::into),
            body: "Needs your input & more".into(),
            sound: true,
        }
    }

    #[test]
    fn a_server_that_answered_shows_them() {
        assert_eq!(
            Abilities::default().permission(),
            NotificationPermission::Unavailable
        );
        let server = Abilities::server(&["body", "actions", "body-markup"]);
        assert_eq!(server.permission(), NotificationPermission::Allowed);
        assert!(server.clicks && server.markup && !server.subtitle);
        let plain = Abilities::server::<&str>(&[]);
        assert_eq!(plain.permission(), NotificationPermission::Allowed);
        assert!(!plain.clicks && !plain.markup);
    }

    #[test]
    fn the_subtitle_follows_the_body_where_there_is_no_line_for_it() {
        let plain = Abilities::server(&["actions"]);
        let Content {
            summary,
            subtitle,
            body,
            sound,
            click,
        } = Content::new(&notification(Some("maxhq on <pc>")), plain);
        assert_eq!(summary, "Fix <the> merge");
        assert_eq!(subtitle, None);
        assert_eq!(body, "Needs your input & more\nmaxhq on <pc>");
        assert!(sound && click);

        let toasts = Abilities {
            shows: true,
            subtitle: true,
            ..Abilities::default()
        };
        let content = Content::new(&notification(Some("maxhq on <pc>")), toasts);
        assert_eq!(content.subtitle.as_deref(), Some("maxhq on <pc>"));
        assert_eq!(content.body, "Needs your input & more");
        assert!(!content.click);

        let content = Content::new(&notification(None), plain);
        assert_eq!(content.body, "Needs your input & more");
    }

    #[test]
    fn markup_is_escaped_only_where_the_body_is_markup() {
        let markup = Abilities::server(&["body-markup"]);
        let content = Content::new(&notification(Some("maxhq on <pc>")), markup);
        assert_eq!(
            content.body,
            "Needs your input &amp; more\nmaxhq on &lt;pc&gt;"
        );
        // The summary is never markup.
        assert_eq!(content.summary, "Fix <the> merge");
    }
}
