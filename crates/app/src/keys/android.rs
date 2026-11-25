// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

//! The Android phone's keys. The app's `dev.maxhq.core.MaxhqKeys` Kotlin
//! object keeps each one as a file in the app's no-backup folder, sealed
//! with AES-256-GCM under a key the Android Keystore holds and never lets
//! out, readable after the first unlock so the app works in the
//! background. Rust reaches it over JNI, and the push service reads the
//! push key through the same object without Rust.

use std::sync::OnceLock;

use jni::errors::Error;
use jni::objects::{JByteArray, JClass, JString};
use jni::refs::Global;
use jni::{jni_sig, jni_str, Env, JValue, JavaVM};
use zeroize::Zeroizing;

use super::{Key, KeyStore};

/// `MaxhqKeys`, found while the app loads the core. A thread Rust attaches
/// later only finds the system's classes, so it couldn't look it up then.
static KEYS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// The keys `MaxhqKeys` keeps. The app sets it up with its context when it
/// makes the core's module, before JS can start the core.
pub struct AndroidStore;

impl AndroidStore {
    /// Finds `MaxhqKeys` through `vm`. The core's `JNI_OnLoad` calls it on
    /// the thread that loads the library, the one place a lookup sees the
    /// app's own classes.
    pub fn attach(vm: &JavaVM) -> Result<(), String> {
        let keys = vm
            .attach_current_thread(|env| {
                let class = env.find_class(jni_str!("dev/maxhq/core/MaxhqKeys"))?;
                env.new_global_ref(class)
            })
            .map_err(|error| format!("Couldn't find the phone's key store: {}", reason(&error)))?;
        let _ = KEYS.set(keys);
        Ok(())
    }
}

impl KeyStore for AndroidStore {
    fn load(&self, key: Key) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        call(key, "read", |env, keys, account| {
            let found = env
                .call_static_method(
                    keys,
                    jni_str!("load"),
                    jni_sig!("(Ljava/lang/String;)[B"),
                    &[JValue::from(account)],
                )?
                .l()?;
            if found.is_null() {
                return Ok(None);
            }
            let found = env.cast_local::<JByteArray>(found)?;
            let bytes = Zeroizing::new(env.convert_byte_array(&found)?);
            clear(env, &found, bytes.len())?;
            Ok(Some(bytes))
        })
    }

    fn save(&self, key: Key, bytes: &[u8]) -> Result<(), String> {
        call(key, "keep", |env, keys, account| {
            let array = env.byte_array_from_slice(bytes)?;
            env.call_static_method(
                keys,
                jni_str!("save"),
                jni_sig!("(Ljava/lang/String;[B)V"),
                &[JValue::from(account), JValue::from(&array)],
            )?
            .v()?;
            clear(env, &array, bytes.len())
        })
    }

    fn forget(&self, key: Key) -> Result<(), String> {
        call(key, "forget", |env, keys, account| {
            env.call_static_method(
                keys,
                jni_str!("forget"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::from(account)],
            )?
            .v()
        })
    }
}

/// Runs `body` with `MaxhqKeys` and `key`'s account as a Java string, on
/// this thread attached to the VM. Each call has a local frame of its own,
/// so the references it makes go when it ends, even on the JS thread,
/// which JNI never returns to. A Java exception comes back as the error,
/// which names what the call was `doing` to which key.
fn call<T>(
    key: Key,
    doing: &str,
    body: impl FnOnce(&mut Env, &Global<JClass<'static>>, &JString) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let failed = |reason: String| format!("Couldn't {doing} {}: {reason}", key.what());
    let keys = KEYS
        .get()
        .ok_or_else(|| failed("the app didn't load its key store".to_owned()))?;
    let vm = JavaVM::singleton().map_err(|error| failed(reason(&error)))?;
    vm.attach_current_thread(|env| {
        let account = env.new_string(key.account())?;
        body(env, keys, &account)
    })
    .map_err(|error| failed(reason(&error)))
}

/// Writes zeros over the Java array that held a key, since the collector
/// frees it whenever it likes.
fn clear(env: &Env, array: &JByteArray, len: usize) -> jni::errors::Result<()> {
    array.set_region(env, 0, &vec![0; len])
}

/// Why a call failed: the Kotlin exception's message where it threw one,
/// which never holds a key.
fn reason(error: &Error) -> String {
    match error {
        Error::CaughtJavaException { name, msg, .. } if msg.is_empty() => name.clone(),
        Error::CaughtJavaException { msg, .. } => msg.clone(),
        error => error.to_string(),
    }
}
