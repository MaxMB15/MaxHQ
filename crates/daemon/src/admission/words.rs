// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What an admission card says, laid out for the app and in plain words
//! for the alert and for a client that doesn't lay out consent.

use jiff::tz::TimeZone;
use maxhq_protocol::ids::AdmissionId;
use maxhq_protocol::time::Timestamp;
use maxhq_protocol::timeline::{Consent, ConsentFact, Question, QuestionOption};

use super::facts::{Facts, Transport};
use super::Ask;
use crate::app_keys::clean_label;
use crate::codesign::Signed;

/// The answer that keeps the program out, which every card offers first.
pub const DONT_ALLOW: &str = "Don't allow";
/// The answer that lets a program in until it disconnects.
const ALLOW: &str = "Allow until it disconnects";
/// The answer that trusts a program's key.
const TRUST: &str = "Trust its key";
/// The longest "Says it is" kept, in characters.
const SAYS_MAX: usize = 100;

/// The device a card is on.
pub struct Host<'a> {
    /// Its name.
    pub device: &'a str,
    /// Whether it's a Mac.
    pub mac: bool,
    /// The time zone the card's times are in.
    pub zone: TimeZone,
}

impl<'a> Host<'a> {
    /// The device this `maxd` runs on, called `device`.
    pub fn here(device: &'a str) -> Self {
        Self {
            device,
            mac: cfg!(target_os = "macos"),
            zone: TimeZone::system(),
        }
    }
}

/// What a card says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Words {
    /// What it asks.
    pub title: String,
    /// What could happen.
    pub risk: String,
    /// What the device knows about the program, in order.
    pub facts: Vec<ConsentFact>,
    /// The answer that lets the program in. [`DONT_ALLOW`] comes before
    /// it.
    pub allow: String,
}

impl Words {
    /// The risk and the facts, one per line.
    pub fn body(&self) -> String {
        let mut body = self.risk.clone();
        body.push('\n');
        for fact in &self.facts {
            body.push('\n');
            body.push_str(&fact.label);
            body.push_str(": ");
            body.push_str(&fact.value);
        }
        body
    }

    /// Everything the card says, in plain words.
    pub fn text(&self) -> String {
        format!("{}\n\n{}", self.title, self.body())
    }

    /// The card as a question with its consent laid out. There's no
    /// remember box: a yes lasts as long as its connection, or as long as
    /// the device trusts the key.
    pub fn question(&self, id: AdmissionId) -> Question {
        let option = |label: &str| QuestionOption {
            label: label.to_owned(),
            description: None,
        };
        Question {
            id: id.to_string(),
            header: None,
            text: self.text(),
            options: vec![option(DONT_ALLOW), option(&self.allow)],
            multi: false,
            other: false,
            secret: false,
            consent: Some(Consent {
                title: self.title.clone(),
                risk: self.risk.clone(),
                changes: Vec::new(),
                facts: self.facts.clone(),
                rows: Vec::new(),
                remember: None,
                edit: None,
            }),
        }
    }
}

/// The card on `host` about a program that asks for `ask`, as `facts`
/// describe it, while `waiting` more were refused a card.
pub fn card(host: &Host<'_>, ask: &Ask, facts: &Facts, waiting: usize) -> Words {
    let device = host.device;
    let (mut risk, allow) = match ask {
        Ask::Admit if facts.phone => {
            let risk = format!(
                "A program connected to maxd on {device} as a paired phone, without showing it \
                 holds that phone's key. It may be an older MaxHQ phone app that hasn't \
                 updated. It may also be an agent on {device} trying to answer its own \
                 questions or change what it may run. If you allow it, it can do what a paired \
                 phone can until it disconnects."
            );
            (risk, ALLOW)
        }
        Ask::Admit => {
            let risk = format!(
                "A program connected to maxd on {device} without MaxHQ's app key. It may be an \
                 older MaxHQ that hasn't updated. It may also be an agent on {device} trying to \
                 answer its own questions, change what it may run or make keys. If you allow \
                 it, it can do anything the app can until it disconnects."
            );
            (risk, ALLOW)
        }
        Ask::Enroll { .. } => {
            let risk = format!(
                "A program connected to maxd on {device} with an app key {device} doesn't trust, \
                 and asked {device} to trust it. It may be MaxHQ on another Mac of yours, or on a \
                 Mac that lost its key. It may also be an agent on {device} trying to answer its \
                 own questions, change what it may run or make keys. If you allow it, {device} \
                 trusts that key until you remove it, so the program can do anything the app can, \
                 now and each time it connects."
            );
            (risk, TRUST)
        }
    };
    if facts.same_mac {
        risk.push_str(
            " MaxHQ is already connected from this Mac, so this is most likely not MaxHQ.",
        );
    }

    let mut list = vec![
        fact("Device", device),
        fact("Says it is", &says(facts.says.as_deref())),
        fact("How it reached maxd", &reached(host, facts)),
    ];
    if let Some(program) = &facts.program {
        let path = program.path.as_ref().map(|path| path.display().to_string());
        list.push(fact("Program", path.as_deref().unwrap_or("Couldn't tell")));
        let signed = match program.signed {
            Signed::Yours => "Your certificate signed it",
            Signed::NotYours => "Your certificate didn't sign it",
            Signed::Unchecked => "Not checked, since this maxd isn't signed",
        };
        list.push(fact("Signature", signed));
    }
    if let Ask::Enroll { public_key, label } = ask {
        list.push(fact("Key name", &clean_label(label)));
        list.push(fact("Key", public_key));
    }
    list.push(fact("Asked", &at(facts.asked_at, &host.zone)));
    if waiting > 0 {
        let more = match waiting {
            1 => "1 more program, with no card of its own".to_owned(),
            n => format!("{n} more programs, with no card of their own"),
        };
        list.push(fact("Also waiting", &more));
    }

    Words {
        title: format!("Let this program control {device}?"),
        risk,
        facts: list,
        allow: allow.to_owned(),
    }
}

fn fact(label: &str, value: &str) -> ConsentFact {
    ConsentFact {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

/// What the hello said, on one line and not too long. The program can say
/// anything here.
fn says(said: Option<&str>) -> String {
    let said: String = said
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let said: String = said.trim().chars().take(SAYS_MAX).collect();
    match said.trim_end() {
        "" => "Couldn't tell".to_owned(),
        said => said.to_owned(),
    }
}

/// How the program reached `maxd`.
fn reached(host: &Host<'_>, facts: &Facts) -> String {
    let mut how = match facts.transport {
        Transport::Socket if host.mac => "This Mac's socket".to_owned(),
        Transport::Socket => format!("The socket on {}", host.device),
        Transport::Proxy => "SSH, through maxd proxy".to_owned(),
        Transport::Network => "Network access".to_owned(),
        Transport::Unknown => "Couldn't tell".to_owned(),
    };
    if facts.phone {
        how.push_str(", as a paired phone");
    }
    how
}

/// `time` as a time of day in `zone`, such as "14:03:07 CET".
fn at(time: Timestamp, zone: &TimeZone) -> String {
    let Some(time) = i64::try_from(time.0)
        .ok()
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
    else {
        return "Couldn't tell".to_owned();
    };
    let zoned = time.to_zoned(zone.clone());
    jiff::fmt::strtime::format("%H:%M:%S %Z", &zoned)
        .or_else(|_| jiff::fmt::strtime::format("%H:%M:%S", &zoned))
        .unwrap_or_else(|_| "Couldn't tell".to_owned())
}
