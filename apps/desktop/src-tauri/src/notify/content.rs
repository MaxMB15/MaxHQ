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

/// The group every toast of the app's is in, which taking them off screen
/// goes by along with each one's tag.
#[cfg(windows)]
pub const TOAST_GROUP: &str = "maxhq";

/// The longest tag Windows takes for a toast, in UTF-16 units.
#[cfg(any(windows, test))]
const LONGEST_TAG: usize = 64;

/// The tag for the toast of the notification with `id`, which showing it
/// again replaces and taking it off screen goes by: the ID itself, or a
/// hash of it that stays the same from run to run where it's too long.
#[cfg(any(windows, test))]
pub fn toast_tag(id: &str) -> String {
    if !id.is_empty() && id.encode_utf16().count() <= LONGEST_TAG {
        return id.to_owned();
    }
    // FNV-1a, so the tag outlasts the process and a toast from an earlier
    // run can still be taken off screen.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// The toast for `content`: the agent's title, what happened, and where
/// the agent runs in the smaller line at the bottom. A prompt plays the
/// toast's own sound and the rest are silent.
#[cfg(any(windows, test))]
pub fn toast_xml(content: &Content) -> String {
    let mut xml = String::from(r#"<toast><visual><binding template="ToastGeneric">"#);
    let mut text = |text: &str, attributes: &str| {
        xml.push_str(&format!("<text{attributes}>{}</text>", xml_text(text)));
    };
    text(&content.summary, "");
    text(&content.body, "");
    if let Some(subtitle) = &content.subtitle {
        text(subtitle, r#" placement="attribution""#);
    }
    xml.push_str("</binding></visual>");
    if !content.sound {
        xml.push_str(r#"<audio silent="true"/>"#);
    }
    xml.push_str("</toast>");
    xml
}

/// `text` for an XML text node: the characters XML reads escaped, and the
/// ones it can't hold at all left out, so an agent's text can't break the
/// toast.
#[cfg(any(windows, test))]
fn xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for each in text.chars() {
        match each {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\t' | '\n' | '\r' => escaped.push(each),
            each if each < ' ' || matches!(each, '\u{fffe}' | '\u{ffff}') => {}
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

    fn toast(sound: bool, subtitle: Option<&str>) -> Content {
        Content {
            summary: "Fix <the> merge".into(),
            subtitle: subtitle.map(Into::into),
            body: "Needs your input & more".into(),
            sound,
            click: true,
        }
    }

    #[test]
    fn a_toast_shows_the_title_then_what_happened_then_where() {
        assert_eq!(
            toast_xml(&toast(true, Some("maxhq on <pc>"))),
            concat!(
                r#"<toast><visual><binding template="ToastGeneric">"#,
                "<text>Fix &lt;the&gt; merge</text>",
                "<text>Needs your input &amp; more</text>",
                r#"<text placement="attribution">maxhq on &lt;pc&gt;</text>"#,
                "</binding></visual></toast>"
            )
        );
        assert_eq!(
            toast_xml(&toast(false, None)),
            concat!(
                r#"<toast><visual><binding template="ToastGeneric">"#,
                "<text>Fix &lt;the&gt; merge</text>",
                "<text>Needs your input &amp; more</text>",
                r#"</binding></visual><audio silent="true"/></toast>"#
            )
        );
    }

    #[test]
    fn what_xml_cant_hold_is_left_out_of_a_toast() {
        assert_eq!(xml_text("a\u{0}b\u{1b}[0m\tc\nd"), "ab[0m\tc\nd");
        assert_eq!(xml_text("\u{fffe}é😀"), "é😀");
    }

    #[test]
    fn a_toast_is_tagged_by_its_id_or_a_steady_hash_of_a_long_one() {
        let id = "needs-input:0199a7c2-5f3e-7d10-8a4b-2c6e9f1d3b57";
        assert_eq!(toast_tag(id), id);
        let long = format!("inbox:{}", "x".repeat(80));
        let tag = toast_tag(&long);
        assert_eq!(tag.len(), 16);
        assert_eq!(tag, toast_tag(&long), "the same every time");
        assert_ne!(tag, toast_tag(&format!("{long}y")));
        assert_eq!(toast_tag(""), "cbf29ce484222325");
        assert_eq!(toast_tag(&"x".repeat(LONGEST_TAG)), "x".repeat(LONGEST_TAG));
    }
}
