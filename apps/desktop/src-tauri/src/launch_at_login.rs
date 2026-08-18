//! Optional macOS login item. Other platforms persist the setting but do not
//! register an OS autostart. Unpackaged `cargo run` builds have no `.app`
//! bundle; enabling then is a no-op for the OS (setting still saves).

use anyhow::Result;
use std::path::PathBuf;

const LABEL: &str = "ai.pulsar.desktop";

pub fn apply(enabled: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        apply_macos(enabled)
    }
    #[cfg(not(target_os = "macos"))]
    {
        if enabled {
            anyhow::bail!("Launch at login is only available on macOS");
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut p = exe.as_path();
    loop {
        if p.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(p.to_path_buf());
        }
        p = p.parent()?;
    }
}

#[cfg(target_os = "macos")]
fn apply_macos(enabled: bool) -> Result<()> {
    let path = plist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !enabled {
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{}/{}", macos_uid(), LABEL)])
            .status();
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let Some(bundle) = app_bundle_path() else {
        tracing::warn!(
            target: "launch_at_login",
            "no .app bundle; login item skipped (unpackaged build)"
        );
        return Ok(());
    };
    let bundle_str = bundle.display().to_string();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/open</string>
    <string>-a</string>
    <string>{bundle_str}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    );
    std::fs::write(&path, xml)?;
    let _ = std::process::Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{}", macos_uid())])
        .arg(&path)
        .status();
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_uid() -> u32 {
    libc_uid()
}

#[cfg(target_os = "macos")]
fn libc_uid() -> u32 {
    // Avoid a libc crate dep: parse `id -u`.
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(501)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_is_always_ok() {
        apply(false).unwrap();
    }
}
