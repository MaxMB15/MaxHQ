// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Max Boksem

use std::{env, fs, path::Path};

/// The Common Controls v6 dependency Tauri gives the app on Windows.
const TEST_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn main() {
    tauri_build::build();

    // Tauri embeds its manifest in the app only. A test that links the
    // dialog code without it imports what only Common Controls v6 has, and
    // Windows won't start it (STATUS_ENTRYPOINT_NOT_FOUND).
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest =
            Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("tests.manifest");
        fs::write(&manifest, TEST_MANIFEST).expect("write the test manifest");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
