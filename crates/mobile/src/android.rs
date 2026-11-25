// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the core does as Android loads it. The core is a static library
//! inside the React Native module's `libmaxhq-core.so`, which keeps the
//! core's symbols to itself, so the module's C++ adapter is the one that
//! exports `JNI_OnLoad`, and it calls this.

use jni::sys::{jint, JNI_VERSION_1_6};
use jni::JavaVM;

/// Hands the VM to the key store while the module's class loads the
/// library, the one time a lookup sees the app's classes. A store that
/// isn't found leaves the core to say so when it starts, since failing
/// here would stop the app from loading at all. Answers the JNI version
/// the library needs, for `JNI_OnLoad` to return.
///
/// # Safety
///
/// `vm` is the pointer the VM passed to `JNI_OnLoad`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn maxhq_mobile_on_load(vm: *mut jni::sys::JavaVM) -> jint {
    // SAFETY: the caller passes the VM's own pointer, as above.
    let vm = unsafe { JavaVM::from_raw(vm) };
    if let Err(error) = maxhq_app::keys::AndroidStore::attach(&vm) {
        tracing::error!(%error, "couldn't set up the phone's key store");
    }
    JNI_VERSION_1_6
}
