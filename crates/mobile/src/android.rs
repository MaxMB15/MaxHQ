// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! What the core does as Android loads it. The core is a static library
//! inside the React Native module's `libmaxhq-core.so`, whose CMake build
//! keeps this `JNI_OnLoad` with `-u` so the VM finds it there.

use std::ffi::c_void;

use jni::sys::{jint, JNI_VERSION_1_6};
use jni::JavaVM;

/// Hands the VM to the key store while the module's class loads the
/// library, the one time a lookup sees the app's classes. A store that
/// isn't found leaves the core to say so when it starts, since failing
/// here would stop the app from loading at all.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut jni::sys::JavaVM, _reserved: *mut c_void) -> jint {
    // SAFETY: the VM calls JNI_OnLoad with a valid pointer to itself.
    let vm = unsafe { JavaVM::from_raw(vm) };
    if let Err(error) = maxhq_app::keys::AndroidStore::attach(&vm) {
        tracing::error!(%error, "couldn't set up the phone's key store");
    }
    JNI_VERSION_1_6
}
