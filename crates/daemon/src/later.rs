// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! A handle filled in once the thing it stands for exists. Some of
//! `maxd`'s parts need each other both ways: the guest links hand guest
//! messages to the transfers, and the transfers send through the guest
//! links. The transfers are built with a [`Later`] and get the links
//! through [`Later::set`].

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use maxhq_protocol::ids::WorkspaceId;

use crate::incus::InstanceName;
use crate::transfer::{self, GuestOut};
use crate::workspace::GuestLinks;
use futures::future::BoxFuture;

/// Something set once, after the parts that hold it were built. Clones
/// share it.
pub struct Later<T>(Arc<OnceLock<T>>);

impl<T> Later<T> {
    /// Not set yet.
    pub fn new() -> Self {
        Self(Arc::new(OnceLock::new()))
    }

    /// Sets it. A second set is ignored, so the first one stays.
    pub fn set(&self, value: T) {
        let _ = self.0.set(value);
    }

    /// What was set, if anything has been.
    pub fn get(&self) -> Option<&T> {
        self.0.get()
    }
}

impl<T> Clone for Later<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Later<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// No guest links until set, or once they're gone. The handle is weak, so
/// links that hold the transfers don't keep themselves alive through it.
impl<T: transfer::Guests> transfer::Guests for Later<Weak<T>> {
    fn link(&self, workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>> {
        self.get()?.upgrade()?.link(workspace)
    }
}

/// No guest links until set, or once they're gone, for the macOS test VM,
/// which `maxd` builds before its links.
impl<T: GuestLinks> GuestLinks for Later<Weak<T>> {
    fn open(&self, workspace: WorkspaceId, instance: &InstanceName, repo: Option<&str>) {
        if let Some(links) = self.get().and_then(Weak::upgrade) {
            links.open(workspace, instance, repo);
        }
    }

    fn close(&self, workspace: WorkspaceId) {
        if let Some(links) = self.get().and_then(Weak::upgrade) {
            links.close(workspace);
        }
    }

    fn wait_up(
        &self,
        workspace: WorkspaceId,
        wait: Duration,
    ) -> BoxFuture<'_, Result<Option<String>, String>> {
        let links = self.get().and_then(Weak::upgrade);
        Box::pin(async move {
            match links {
                Some(links) => links.wait_up(workspace, wait).await,
                None => Ok(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use maxhq_protocol::guest::{GuestToHost, HostToGuest};
    use maxhq_protocol::ids::TransferId;
    use maxhq_wire::Closed;

    use super::*;
    use crate::session::clock::BoxFuture;
    use crate::transfer::MakeRequest;

    #[test]
    fn the_first_set_stays_and_clones_share_it() {
        let later = Later::new();
        let other = later.clone();
        assert_eq!(later.get(), None);
        other.set(1);
        later.set(2);
        assert_eq!(later.get(), Some(&1));
        assert_eq!(other.get(), Some(&1));
    }

    /// Guest links that always have one, which takes nothing.
    struct Always;

    impl transfer::Guests for Always {
        fn link(&self, _workspace: WorkspaceId) -> Option<Arc<dyn GuestOut>> {
            Some(Arc::new(Always))
        }
    }

    impl GuestOut for Always {
        fn message(&self, _message: HostToGuest) -> BoxFuture<Result<(), Closed>> {
            Box::pin(async { Err(Closed) })
        }

        fn chunk(
            &self,
            _transfer: TransferId,
            _offset: u64,
            _data: Vec<u8>,
        ) -> BoxFuture<Result<(), Closed>> {
            Box::pin(async { Err(Closed) })
        }

        fn request(
            &self,
            _wait: Duration,
            _make: MakeRequest,
        ) -> BoxFuture<Result<GuestToHost, String>> {
            Box::pin(async { Err("closed".to_owned()) })
        }
    }

    #[test]
    fn links_answer_only_while_set_and_alive() {
        let workspace = WorkspaceId::derive("w");
        let later: Later<Weak<Always>> = Later::new();
        assert!(transfer::Guests::link(&later, workspace).is_none());
        let links = Arc::new(Always);
        later.set(Arc::downgrade(&links));
        assert!(transfer::Guests::link(&later, workspace).is_some());
        drop(links);
        assert!(transfer::Guests::link(&later, workspace).is_none());
    }
}
