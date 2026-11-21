//! Compiles `app.rc` (icon, application manifest and version information) with
//! the Windows SDK's resource compiler and links the result into the exe. The
//! version information comes from `Cargo.toml` and `LICENSE`, written to
//! `version.rc` in `OUT_DIR`.

#![allow(clippy::expect_used, reason = "a build script stops the build by panicking, with the reason")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// `rc.exe` from `RC`, or the newest Windows 10/11 SDK.
fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(rc);
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let program_files = std::env::var_os("ProgramFiles(x86)").unwrap_or_else(|| r"C:\Program Files (x86)".into());
    let bin = Path::new(&program_files).join(r"Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&bin)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("10.")))
        .collect();
    // Version folders compare correctly as numbers per component.
    versions.sort_by_key(|p| {
        p.file_name().map(|n| n.to_string_lossy().split('.').map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
    });
    versions.iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.is_file())
}

/// A string for a resource script: in quotes, quotes doubled.
fn rc_string(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The copyright line of `LICENSE` ("Copyright (c) 2026 Name"), so its year
/// and holder are written once.
fn copyright() -> String {
    std::fs::read_to_string("LICENSE")
        .ok()
        .and_then(|l| l.lines().find(|l| l.starts_with("Copyright")).map(|l| l.trim().to_string()))
        .unwrap_or_default()
}

/// The VERSIONINFO resource: Explorer shows it under Properties -> Details,
/// Task Manager shows its description. Numbers instead of `winver.h`'s names:
/// rc.exe finds that header only in a Visual Studio prompt.
fn version_rc() -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let num = |k: &str| env(k).parse::<u16>().unwrap_or(0);
    let version = format!(
        "{},{},{},0",
        num("CARGO_PKG_VERSION_MAJOR"),
        num("CARGO_PKG_VERSION_MINOR"),
        num("CARGO_PKG_VERSION_PATCH")
    );
    // A pre-release (1.2.0-rc.1) says so: VS_FF_PRERELEASE.
    let flags = if env("CARGO_PKG_VERSION_PRE").is_empty() { "0x0" } else { "0x2" };
    let name = env("CARGO_PKG_NAME");
    let comments = format!(
        "{}. {}. An independent project, not affiliated with Anthropic, OpenAI, GitHub, Microsoft or GitLab.",
        env("CARGO_PKG_DESCRIPTION"),
        env("CARGO_PKG_REPOSITORY")
    );
    let strings = [
        ("CompanyName", env("CARGO_PKG_AUTHORS").replace(':', ", ")),
        ("FileDescription", "Trayward".to_string()),
        ("FileVersion", env("CARGO_PKG_VERSION")),
        ("InternalName", name.clone()),
        ("LegalCopyright", format!("{} - {} License", copyright(), env("CARGO_PKG_LICENSE"))),
        ("OriginalFilename", format!("{name}.exe")),
        ("ProductName", "Trayward".to_string()),
        ("ProductVersion", env("CARGO_PKG_VERSION")),
        ("Comments", comments),
    ];
    let mut lines = vec![
        // Plain ASCII: rc.exe reads an included file in the system code page.
        "1 VERSIONINFO".to_string(),
        format!("FILEVERSION {version}"),
        format!("PRODUCTVERSION {version}"),
        // VS_FFI_FILEFLAGSMASK, VOS_NT_WINDOWS32, VFT_APP.
        "FILEFLAGSMASK 0x3F".into(),
        format!("FILEFLAGS {flags}"),
        "FILEOS 0x40004".into(),
        "FILETYPE 0x1".into(),
        "FILESUBTYPE 0x0".into(),
        "BEGIN".into(),
        "  BLOCK \"StringFileInfo\"".into(),
        "  BEGIN".into(),
        // US English, Unicode.
        "    BLOCK \"040904B0\"".into(),
        "    BEGIN".into(),
    ];
    lines.extend(strings.iter().map(|(k, v)| format!("      VALUE {}, {}", rc_string(k), rc_string(v))));
    lines.extend(
        [
            "    END",
            "  END",
            "  BLOCK \"VarFileInfo\"",
            "  BEGIN",
            "    VALUE \"Translation\", 0x409, 1200",
            "  END",
            "END",
            "",
        ]
        .map(String::from),
    );
    lines.join(
        "
",
    )
}

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=LICENSE");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=RC");

    let rc = find_rc().expect("rc.exe not found: install the Windows SDK or set RC to its path");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("version.rc"), version_rc()).expect("writing version.rc failed");
    let out = out_dir.join("app.res");
    // /i: app.rc includes version.rc from OUT_DIR.
    let status = Command::new(&rc)
        .args(["/nologo", "/i"])
        .arg(&out_dir)
        .arg("/fo")
        .arg(&out)
        .arg("app.rc")
        .status()
        .expect("running rc.exe failed");
    assert!(status.success(), "rc.exe failed on app.rc");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
