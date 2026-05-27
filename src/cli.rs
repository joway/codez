//! The `codez` shell-command launcher: detect whether it's installed, install
//! it (with an admin prompt when needed), and remember if the user dismissed
//! the startup install prompt.

use std::path::PathBuf;

const TARGET: &str = "/usr/local/bin/codez";

/// Whether the `codez` launcher is present at the canonical location.
pub fn installed() -> bool {
    std::path::Path::new(TARGET).exists()
}

/// Install a `codez` launcher that opens this very app binary. Writes directly
/// if possible, otherwise asks for admin rights via a native macOS prompt.
/// Returns the install path on success.
pub fn install() -> Result<String, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate app binary: {e}"))?
        .to_string_lossy()
        .into_owned();
    let shim = format!(
        "#!/bin/sh\n\
         target=\"${{1:-$PWD}}\"\n\
         case \"$target\" in /*) ;; *) target=\"$PWD/$target\" ;; esac\n\
         \"{exe}\" \"$target\" >/dev/null 2>&1 &\n"
    );

    if write_executable(TARGET, &shim).is_ok() {
        return Ok(TARGET.to_string());
    }
    install_with_admin(TARGET, &shim)?;
    Ok(TARGET.to_string())
}

fn write_executable(path: &str, contents: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

/// Stage the shim in a temp file, then move it into place with administrator
/// privileges (pops the standard macOS password dialog via osascript).
fn install_with_admin(target: &str, contents: &str) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("codez-cli-shim");
    std::fs::write(&tmp, contents).map_err(|e| e.to_string())?;
    let tmp = tmp.to_string_lossy().into_owned();

    let cmd = format!(
        "mkdir -p /usr/local/bin && /bin/cp '{tmp}' '{target}' && /bin/chmod 755 '{target}'"
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        cmd.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("osascript failed: {e}"));
    let _ = std::fs::remove_file(&tmp);

    let output = output?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    // -128 = user cancelled the authorization dialog.
    if err.contains("-128") {
        Err("cancelled".to_string())
    } else {
        Err(err.trim().to_string())
    }
}

// ---------------- "don't remind me" marker ----------------

fn marker_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".codez").join("skip-cli-prompt"))
}

/// True if the user asked not to be prompted to install the CLI again.
pub fn prompt_dismissed() -> bool {
    marker_path().is_some_and(|p| p.exists())
}

/// Persist the user's choice to stop showing the startup install prompt.
pub fn dismiss_prompt() {
    if let Some(path) = marker_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, b"");
    }
}
