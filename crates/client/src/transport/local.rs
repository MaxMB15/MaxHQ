// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The `maxd` on this machine, over its local socket: a Unix socket, or on
//! Windows the named pipe `maxhq_os::ipc` names after the same path, whose
//! server it checks runs as this user.

use std::io;
use std::path::Path;

use maxhq_os::ipc::LocalStream;

use super::{Conn, DialError};

/// Connects to a `maxd` socket on this machine.
pub(crate) async fn dial_local(socket: &Path) -> Result<Conn, DialError> {
    match LocalStream::connect(socket).await {
        Ok(stream) => {
            let (reader, writer) = stream.into_split();
            Ok(Conn::new(Box::new(reader), Box::new(writer)))
        }
        Err(err) => Err(DialError::refused(match err.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                "maxd isn't running".to_owned()
            }
            io::ErrorKind::PermissionDenied => "maxd's socket belongs to someone else".to_owned(),
            _ => format!("can't open maxd's socket: {err}"),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_sockets_that_arent_there_mean_maxd_isnt_running() {
        let path = std::env::temp_dir().join(format!("maxhq-missing-{}.sock", std::process::id()));
        let err = dial_local(&path).await.err().unwrap();
        assert_eq!(err, DialError::refused("maxd isn't running"));
    }
}
