// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! Each webhook's signing secret, in a file of its own. It's read only
//! to check a request's signature and never goes back to a client.

use std::io;

use maxhq_protocol::ids::TaskId;
use maxhq_protocol::Secret;

use super::Secrets;
use crate::hooks::HookSecrets;

/// The file for the signing secret of `task`'s webhook.
fn file(task: TaskId) -> String {
    format!("hook-{task}.secret")
}

impl HookSecrets for Secrets {
    fn set(&self, task: TaskId, secret: &Secret) -> io::Result<()> {
        self.inner
            .folder
            .write(&file(task), secret.expose().as_bytes())
    }

    fn get(&self, task: TaskId) -> io::Result<Option<Secret>> {
        self.inner.folder.read(&file(task))
    }

    fn remove(&self, task: TaskId) -> io::Result<()> {
        self.inner.folder.remove(&file(task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::testing::hub;
    use crate::secrets::DIR;
    use crate::testing::TempDir;

    #[test]
    fn keeps_a_hooks_secret_until_it_goes() {
        let temp = TempDir::new();
        let store = Secrets::open(temp.path().join(DIR), hub()).unwrap();
        let task = TaskId::generate();
        assert!(store.get(task).unwrap().is_none());
        store.set(task, &Secret::new("shh".into())).unwrap();
        assert_eq!(store.get(task).unwrap().unwrap().expose(), "shh");
        store.remove(task).unwrap();
        store.remove(task).unwrap();
        assert!(store.get(task).unwrap().is_none());
    }
}
