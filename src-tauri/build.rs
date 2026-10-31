use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-env-changed=CLARO_SIGN_IDENTITY");
    println!(r#"cargo:rustc-check-cfg=cfg(feature,values("cargo-clippy"))"#);
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let helper = out.join(if target == "windows" {
        "claro-capture.exe"
    } else {
        "claro-capture"
    });
    if target == "macos" {
        assert!(Command::new("/usr/bin/swiftc")
            .args(["-O", "native/Capture.swift", "-o"])
            .arg(&helper)
            .status()
            .expect("Swift compiler required")
            .success());
    } else if target == "windows" {
        assert!(Command::new("cl.exe")
            .args([
                "/nologo",
                "/O2",
                "/EHsc",
                "/std:c++17",
                "native/Capture.cpp"
            ])
            .arg(format!("/Fo{}", out.join("Capture.obj").display()))
            .arg(format!("/Fe:{}", helper.display()))
            .args([
                "/link",
                "ole32.lib",
                "oleaut32.lib",
                "user32.lib",
                "uiautomationcore.lib"
            ])
            .status()
            .expect("MSVC required")
            .success());
    }
    if target == "macos" {
        if let Ok(identity) = env::var("CLARO_SIGN_IDENTITY") {
            assert!(Command::new("codesign")
                .args([
                    "--force",
                    "--options",
                    "runtime",
                    "--timestamp",
                    "--sign",
                    &identity
                ])
                .arg(&helper)
                .status()
                .expect("codesign required")
                .success());
        }
    }
    println!("cargo:rustc-env=CLARO_CAPTURE={}", helper.display());
    println!("cargo:rustc-check-cfg=cfg(offline_engine)");
    println!("cargo:rerun-if-env-changed=CLARO_OFFLINE_DIR");
    if let Ok(dir) = env::var("CLARO_OFFLINE_DIR") {
        let dir = PathBuf::from(dir);
        let executable = dir.join(if target == "windows" {
            "claro-offline.exe"
        } else {
            "claro-offline"
        });
        let library = dir.join(if target == "windows" {
            "ctranslate2.dll"
        } else {
            "libctranslate2.dylib"
        });
        assert!(
            executable.exists() && library.exists(),
            "Offline runner and library must both exist"
        );
        println!("cargo:rustc-cfg=offline_engine");
        println!(
            "cargo:rustc-env=CLARO_OFFLINE_RUNNER={}",
            executable.display()
        );
        println!(
            "cargo:rustc-env=CLARO_OFFLINE_LIBRARY={}",
            library.display()
        );
    }
    tauri_build::build()
}
