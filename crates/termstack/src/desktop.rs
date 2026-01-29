//! Desktop integration for GNOME and other freedesktop-compliant desktops.
//!
//! Installs/uninstalls the .desktop file and icons to ~/.local/share/
//! so that GNOME can display the correct application icon.

use std::fs;
use std::path::PathBuf;

const ICON_256: &[u8] = include_bytes!("../../../assets/icons/hicolor/256x256/apps/termstack.png");

fn local_share() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".local/share"))
}

/// Generate desktop file with absolute icon path
fn generate_desktop_file(icon_path: &str) -> String {
    format!(
        r#"[Desktop Entry]
Name=TermStack
Comment=Content-aware terminal compositor
Exec=termstack
Icon={icon_path}
Terminal=false
Type=Application
Categories=System;TerminalEmulator;
StartupWMClass=termstack
"#
    )
}

/// Install desktop file and icon to ~/.local/share/
pub fn install() -> anyhow::Result<()> {
    let base = local_share()?;

    // Install icon (only need one size with absolute path)
    let icon_path = base.join("icons/termstack.png");
    if let Some(parent) = icon_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&icon_path, ICON_256)?;
    println!("Installed: {}", icon_path.display());

    // Install desktop file with absolute icon path
    let desktop_path = base.join("applications/termstack.desktop");
    if let Some(parent) = desktop_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let desktop_content = generate_desktop_file(icon_path.to_str().unwrap());
    fs::write(&desktop_path, desktop_content)?;
    println!("Installed: {}", desktop_path.display());

    // Update desktop database
    let _ = std::process::Command::new("update-desktop-database")
        .arg(base.join("applications"))
        .status();

    println!("\nDesktop integration installed.");
    Ok(())
}

/// Remove desktop file and icon from ~/.local/share/
pub fn uninstall() -> anyhow::Result<()> {
    let base = local_share()?;

    let files = [
        base.join("applications/termstack.desktop"),
        base.join("icons/termstack.png"),
        // Also clean up old hicolor locations if present
        base.join("icons/hicolor/48x48/apps/termstack.png"),
        base.join("icons/hicolor/64x64/apps/termstack.png"),
        base.join("icons/hicolor/128x128/apps/termstack.png"),
    ];

    for path in &files {
        match fs::remove_file(path) {
            Ok(()) => println!("Removed: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("Failed to remove {}: {}", path.display(), e);
            }
        }
    }

    // Update desktop database
    let _ = std::process::Command::new("update-desktop-database")
        .arg(base.join("applications"))
        .status();

    println!("Desktop integration removed.");
    Ok(())
}
