// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A connection's terminal requests and the keystrokes it sends. A
//! terminal can do anything you can on this device, so only the app and
//! phones that proved their keys reach one, and a phone only as far as
//! [`access::check`] lets it.

use maxhq_protocol::ids::TerminalId;
use maxhq_protocol::message::ErrorCode;
use maxhq_protocol::ClientMessage;
use tracing::debug;

use super::{ConnError, Connection};
use crate::terminals::access::{self, Act, Caller};
use crate::terminals::{Refusal, Terminals};

/// The refusal for a terminal request from a connection that may not make
/// one.
pub(super) const NOT_FOR_TERMINALS: &str = "Terminals answer only the MaxHQ app and phones that \
                                            proved their keys, since a terminal can do anything \
                                            you can on this device.";

/// The refusal for a terminal request to a `maxd` that holds none.
const NO_TERMINALS: &str = "This maxd holds no terminals.";

impl Connection {
    /// Who asks, when this connection may reach terminals at all: the app,
    /// with a key it proved, or a phone that proved its key. Not `maxd
    /// mcp`, a link from another device, a pairing, a connection you let
    /// in from its card without a key, or one that proved nothing.
    pub(super) fn terminal_caller(&self) -> Option<Caller> {
        if !self.yours() {
            return None;
        }
        match self.phone {
            Some(phone) => self.frame_proven.then(|| Caller::Phone {
                phone,
                everything: self.may_do_everything(),
            }),
            None => self.app.is_some().then_some(Caller::App),
        }
    }

    /// The terminals and who asks, or the refusal.
    fn terminals(&self) -> Result<(Terminals, Caller, u64), &'static str> {
        let caller = self.terminal_caller().ok_or(NOT_FOR_TERMINALS)?;
        let terminals = self.sessions.terminals().ok_or(NO_TERMINALS)?.clone();
        let conn = self.client.as_ref().ok_or(NOT_FOR_TERMINALS)?.conn();
        Ok((terminals, caller, conn))
    }

    /// Answers a terminal request.
    pub(super) async fn terminal(&mut self, message: ClientMessage) -> Result<(), ConnError> {
        let (terminals, caller, conn) = match self.terminals() {
            Ok(found) => found,
            Err(reason) => return self.refuse(&message, ErrorCode::NotAllowed, reason).await,
        };
        match message {
            ClientMessage::TerminalOpen { ref kind, .. } => {
                if let Err(reason) = access::check(caller, Act::Open, kind, caller.opener()) {
                    return self.refuse(&message, ErrorCode::NotAllowed, reason).await;
                }
                if let Some(client) = &self.client {
                    client.open_terminal(message, caller.opener());
                }
                Ok(())
            }
            ClientMessage::TerminalAttach { terminal, after } => {
                let wire = self.wire.clone();
                let attached = terminals.attach(conn, caller, wire, terminal, after).await;
                self.terminal_refusal(attached).await
            }
            ClientMessage::TerminalDetach { terminal } => {
                terminals.detach(conn, terminal);
                Ok(())
            }
            ClientMessage::TerminalResize {
                terminal,
                cols,
                rows,
            } => {
                let resized = terminals.resize(conn, caller, terminal, cols, rows);
                self.terminal_refusal(resized).await
            }
            ClientMessage::TerminalInterrupt { key, terminal } => {
                self.outcome(key, terminals.interrupt(caller, terminal))
                    .await
            }
            ClientMessage::TerminalClose {
                key,
                terminal,
                force,
            } => {
                self.outcome(key, terminals.close(caller, terminal, force))
                    .await
            }
            ClientMessage::TerminalRename {
                key,
                terminal,
                title,
            } => {
                self.outcome(key, terminals.rename(caller, terminal, &title))
                    .await
            }
            ClientMessage::TerminalMove { key, terminal } => {
                self.outcome(key, terminals.move_run(caller, terminal))
                    .await
            }
            other => {
                let reason = "not a terminal request";
                self.refuse(&other, ErrorCode::BadMessage, reason).await
            }
        }
    }

    /// Types what a `TerminalInput` frame carries into its terminal. The
    /// bytes go to the holder and nowhere else, so nothing here logs them.
    pub(super) async fn terminal_input(
        &mut self,
        terminal: TerminalId,
        data: Vec<u8>,
    ) -> Result<(), ConnError> {
        let (terminals, caller, conn) = match self.terminals() {
            Ok(found) => found,
            Err(reason) => {
                debug!(%terminal, "refused a terminal's input");
                return self.error(ErrorCode::NotAllowed, reason).await;
            }
        };
        let typed = terminals.input(conn, caller, terminal, data);
        self.terminal_refusal(typed).await
    }

    /// Sends the error for a request that didn't go through, if any.
    async fn terminal_refusal(&mut self, result: Result<(), Refusal>) -> Result<(), ConnError> {
        match result {
            Ok(()) => Ok(()),
            Err(refusal) => {
                let (code, message) = refusal.into_error();
                self.error(code, message).await
            }
        }
    }
}

#[cfg(test)]
mod tests;
