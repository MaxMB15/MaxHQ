// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who may do what with a terminal.
//!
//! Only the app and paired phones reach terminals at all: the server turns
//! everyone else away before a request gets here. The app may do anything.
//! A phone may open a `!` run, watch any terminal and send Ctrl-C to it,
//! and type into, size, close, rename or move the runs it opened itself.
//! The rest, a shell above all, needs its "Let this phone do everything"
//! switch, since what you type into a terminal can be a password.

use maxhq_protocol::ids::PhoneId;
use maxhq_protocol::terminal::TerminalKind;
use serde::{Deserialize, Serialize};

/// The refusal for a shell, to a phone whose switch is off.
pub const PHONE_SHELL: &str = "A phone can't open a shell. To let this phone, turn on Let this \
                               phone do everything for it in the Mac's Settings, under Phones.";

/// The refusal for a terminal a phone didn't open, to a phone whose switch
/// is off.
pub const PHONE_NOT_ITS_OWN: &str = "A phone can only type into the runs it started. To let \
                                     this phone use every terminal, turn on Let this phone do \
                                     everything for it in the Mac's Settings, under Phones.";

/// Who sent a terminal request, as its connection proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The app, with a key this device trusts.
    App,
    /// A paired phone that proved its key.
    Phone {
        /// The phone.
        phone: PhoneId,
        /// Whether the Mac turned on its "Let this phone do everything"
        /// switch.
        everything: bool,
    },
}

impl Caller {
    /// Who a terminal this caller opens says opened it.
    pub fn opener(self) -> Opener {
        match self {
            Caller::App => Opener::App,
            Caller::Phone { phone, .. } => Opener::Phone { phone },
        }
    }
}

/// Who opened a terminal, kept with it for as long as it lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Opener {
    /// The app.
    App,
    /// A phone.
    Phone {
        /// The phone.
        phone: PhoneId,
    },
}

/// What a request does to a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Opens one.
    Open,
    /// Gets its output, stops getting it, or sends it Ctrl-C.
    Watch,
    /// Types into it, or sets its size.
    Type,
    /// Closes, renames or moves it.
    Manage,
}

/// Whether `caller` may `act` on a terminal that runs `kind` and that
/// `opener` opened, or why not.
pub fn check(
    caller: Caller,
    act: Act,
    kind: &TerminalKind,
    opener: Opener,
) -> Result<(), &'static str> {
    let Caller::Phone { phone, everything } = caller else {
        return Ok(());
    };
    if everything {
        return Ok(());
    }
    let run = matches!(kind, TerminalKind::Run { .. });
    match act {
        Act::Open if run => Ok(()),
        Act::Open => Err(PHONE_SHELL),
        Act::Watch => Ok(()),
        Act::Type | Act::Manage if run && opener == (Opener::Phone { phone }) => Ok(()),
        Act::Type | Act::Manage => Err(PHONE_NOT_ITS_OWN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> TerminalKind {
        TerminalKind::Shell
    }

    fn run() -> TerminalKind {
        TerminalKind::Run {
            command: "ls".into(),
            private: false,
        }
    }

    fn phone(name: &str, everything: bool) -> Caller {
        Caller::Phone {
            phone: PhoneId::derive(name),
            everything,
        }
    }

    const ACTS: [Act; 4] = [Act::Open, Act::Watch, Act::Type, Act::Manage];

    #[test]
    fn the_app_may_do_anything() {
        for act in ACTS {
            for kind in [shell(), run()] {
                for opener in [Opener::App, phone("a", false).opener()] {
                    assert_eq!(check(Caller::App, act, &kind, opener), Ok(()));
                }
            }
        }
    }

    #[test]
    fn a_phone_with_its_switch_on_may_do_anything() {
        let caller = phone("a", true);
        for act in ACTS {
            for kind in [shell(), run()] {
                for opener in [Opener::App, phone("b", false).opener()] {
                    assert_eq!(check(caller, act, &kind, opener), Ok(()));
                }
            }
        }
    }

    #[test]
    fn a_phone_opens_runs_but_not_shells() {
        let caller = phone("a", false);
        let opener = caller.opener();
        assert_eq!(check(caller, Act::Open, &run(), opener), Ok(()));
        assert_eq!(check(caller, Act::Open, &shell(), opener), Err(PHONE_SHELL));
    }

    #[test]
    fn a_phone_watches_and_interrupts_any_terminal() {
        let caller = phone("a", false);
        for kind in [shell(), run()] {
            for opener in [Opener::App, phone("b", false).opener(), caller.opener()] {
                assert_eq!(check(caller, Act::Watch, &kind, opener), Ok(()));
            }
        }
    }

    #[test]
    fn a_phone_types_into_and_manages_only_its_own_runs() {
        let caller = phone("a", false);
        for act in [Act::Type, Act::Manage] {
            assert_eq!(check(caller, act, &run(), caller.opener()), Ok(()));
            // A run the app or another phone opened.
            for opener in [Opener::App, phone("b", false).opener()] {
                assert_eq!(check(caller, act, &run(), opener), Err(PHONE_NOT_ITS_OWN));
            }
            // A shell, even one it opened while its switch was on.
            for opener in [Opener::App, caller.opener()] {
                assert_eq!(check(caller, act, &shell(), opener), Err(PHONE_NOT_ITS_OWN));
            }
        }
    }

    #[test]
    fn an_opener_reads_back_as_it_was_kept() {
        for opener in [Opener::App, phone("a", false).opener()] {
            let json = serde_json::to_string(&opener).unwrap();
            assert_eq!(serde_json::from_str::<Opener>(&json).unwrap(), opener);
        }
    }
}
