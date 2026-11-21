// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Who may do what with a terminal.
//!
//! Only the app and paired phones reach terminals at all: the server turns
//! everyone else away before a request gets here. The app may do anything.
//! A phone may watch a `!` run and send Ctrl-C to it, and close, rename or
//! move the runs it opened itself. The rest needs its "Let this phone do
//! everything" switch: opening a run or a shell, since a command can do
//! anything you can on this device, typing into any terminal, since a run
//! can be a shell too, and watching a shell, since what it shows can be a
//! password.

use maxhq_protocol::ids::PhoneId;
use maxhq_protocol::terminal::TerminalKind;
use serde::{Deserialize, Serialize};

/// The refusal for running a command, typing into a terminal or watching
/// a shell, to a phone whose switch is off.
pub const PHONE_CANT_RUN: &str = "A phone can't run commands, type into a terminal or watch a \
                                  shell. To let this phone, turn on Let this phone do \
                                  everything for it in the Mac's Settings, under Phones.";

/// The refusal for a terminal a phone didn't open, to a phone whose switch
/// is off.
pub const PHONE_NOT_ITS_OWN: &str = "A phone can only close, rename or move the runs it \
                                     started. To let this phone use every terminal, turn on \
                                     Let this phone do everything for it in the Mac's \
                                     Settings, under Phones.";

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
        Act::Watch if run => Ok(()),
        Act::Manage if run && opener == (Opener::Phone { phone }) => Ok(()),
        Act::Manage => Err(PHONE_NOT_ITS_OWN),
        Act::Open | Act::Type | Act::Watch => Err(PHONE_CANT_RUN),
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

    fn session() -> TerminalKind {
        TerminalKind::Session
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
    fn a_phone_opens_and_types_into_nothing() {
        let caller = phone("a", false);
        for act in [Act::Open, Act::Type] {
            for kind in [shell(), run(), session()] {
                for opener in [Opener::App, caller.opener()] {
                    assert_eq!(check(caller, act, &kind, opener), Err(PHONE_CANT_RUN));
                }
            }
        }
    }

    #[test]
    fn a_phone_watches_and_interrupts_runs_but_not_shells() {
        let caller = phone("a", false);
        for opener in [Opener::App, phone("b", false).opener(), caller.opener()] {
            assert_eq!(check(caller, Act::Watch, &run(), opener), Ok(()));
            for kind in [shell(), session()] {
                assert_eq!(
                    check(caller, Act::Watch, &kind, opener),
                    Err(PHONE_CANT_RUN)
                );
            }
        }
    }

    #[test]
    fn a_phone_manages_only_its_own_runs() {
        let caller = phone("a", false);
        assert_eq!(check(caller, Act::Manage, &run(), caller.opener()), Ok(()));
        // A run the app or another phone opened.
        for opener in [Opener::App, phone("b", false).opener()] {
            assert_eq!(
                check(caller, Act::Manage, &run(), opener),
                Err(PHONE_NOT_ITS_OWN)
            );
        }
        // A shell or session, even one it opened while its switch was on.
        for kind in [shell(), session()] {
            for opener in [Opener::App, caller.opener()] {
                assert_eq!(
                    check(caller, Act::Manage, &kind, opener),
                    Err(PHONE_NOT_ITS_OWN)
                );
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
