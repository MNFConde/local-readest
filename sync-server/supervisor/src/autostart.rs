//! Boot-autostart registration (⑩ P3). Two orthogonal switches by design:
//! "does the process run" and "is it registered to autostart" never couple —
//! a temporary stop (stop.bat / `systemctl --user stop`) survives reboots
//! until the next logon brings it back; `uninstall --keep-running` removes
//! the registration while leaving the instance alone; plain `uninstall`
//! removes the registration AND stops the running instance (main.rs drives
//! the stop through the observation socket's shutdown command).
//!
//! Platform choices:
//!   * Windows: a .lnk in the user's Startup folder (`shell:startup`),
//!     minimized, target `<exe> --headless`. No admin rights, visible in the
//!     folder, deletable by hand. Deliberately NOT HKCU Run (invisible to
//!     users) and NOT a service shim (an autostart needing pre-logon belongs
//!     to the already-documented NSSM route).
//!   * Linux: a systemd **user** unit + `systemctl --user enable --now` — no
//!     root. On a headless server add `loginctl enable-linger <user>` so the
//!     unit runs without a login session (printed as a hint on failure).
//!
//! Autostarted instances always run `--headless`: their control surfaces are
//! the tray, the admin page and an attached TUI — never their own terminal.

/// Unit/shortcut base name (the systemd unit is `<name>.service`; the
/// Windows shortcut title is spelled out in the PowerShell script).
#[cfg_attr(windows, allow(dead_code))]
pub const AUTOSTART_NAME: &str = "readest-supervisor";

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, Context, Result};

    /// One PowerShell script does install / uninstall / query, parameterized
    /// by READEST_SUP_MODE — the folder must be resolved via
    /// GetFolderPath('Startup') because OneDrive profiles redirect it.
    const PS_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$startup = [Environment]::GetFolderPath('Startup')
$lnk = Join-Path $startup 'Readest Server.lnk'
switch ($env:READEST_SUP_MODE) {
  'query'    { if (Test-Path -LiteralPath $lnk) { Write-Output 'yes' } else { Write-Output 'no' } }
  'install'  {
    $ws = New-Object -ComObject WScript.Shell
    $s = $ws.CreateShortcut($lnk)
    $s.TargetPath = $env:READEST_SUP_EXE
    $s.Arguments = '--headless'
    $s.WorkingDirectory = $env:READEST_SUP_DIR
    $s.WindowStyle = 7
    $s.Description = 'Readest self-host server supervisor'
    $s.Save()
    Write-Output $lnk
  }
  'uninstall' { if (Test-Path -LiteralPath $lnk) { Remove-Item -LiteralPath $lnk; Write-Output 'removed' } else { Write-Output 'absent' } }
  default { throw "bad mode" }
}
"#;

    fn run_ps(mode: &str, exe: Option<&std::path::Path>) -> Result<String> {
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", PS_SCRIPT])
            .env("READEST_SUP_MODE", mode);
        if let Some(exe) = exe {
            cmd.env("READEST_SUP_EXE", exe).env(
                "READEST_SUP_DIR",
                exe.parent().unwrap_or_else(|| std::path::Path::new(".")),
            );
        }
        let out = cmd
            .output()
            .map_err(|e| anyhow!("cannot launch powershell: {e}"))?;
        if !out.status.success() {
            return Err(anyhow!(
                "powershell {} failed: {}",
                mode,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn install(exe: &std::path::Path) -> Result<String> {
        // "powershell" resolves via PATH on every stock Windows; App-Execution-Alias-less
        // Server Core still ships it in System32\WindowsPowerShell.
        run_ps("install", Some(exe)).context("create startup shortcut")
    }

    /// Returns true when a registration actually existed and was removed.
    pub fn unregister(exe_dir: &std::path::Path) -> Result<bool> {
        let _ = exe_dir;
        match run_ps("uninstall", None).context("remove startup shortcut")? {
            s if s == "removed" => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn is_registered(exe_dir: &std::path::Path) -> bool {
        let _ = exe_dir;
        run_ps("query", None).map(|s| s == "yes").unwrap_or(false)
    }
}

#[cfg(unix)]
mod imp {
    use super::AUTOSTART_NAME;
    use anyhow::{anyhow, Context, Result};
    use std::path::PathBuf;

    fn unit_path() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(
            config
                .join("systemd/user")
                .join(format!("{AUTOSTART_NAME}.service")),
        )
    }

    fn systemctl(args: &[&str]) -> Result<std::process::Output> {
        std::process::Command::new("systemctl")
            .args(["--user"])
            .args(args)
            .output()
            .map_err(|e| anyhow!("cannot launch systemctl: {e}"))
    }

    pub fn install(exe: &std::path::Path) -> Result<String> {
        let unit = unit_path().ok_or_else(|| anyhow!("neither HOME nor XDG_CONFIG_HOME set"))?;
        let dir = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
        let body = format!(
            "[Unit]\nDescription=Readest self-host server supervisor\n\n[Service]\nExecStart={} --headless\nWorkingDirectory={}\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
            exe.display(),
            dir.display()
        );
        if let Some(parent) = unit.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        std::fs::write(&unit, body).with_context(|| format!("write {}", unit.display()))?;
        systemctl(&["daemon-reload"]).context("systemctl --user daemon-reload")?;
        let enable = systemctl(&["enable", "--now", AUTOSTART_NAME])
            .context("systemctl --user enable --now")?;
        if !enable.status.success() {
            return Err(anyhow!(
                "systemctl --user enable --now failed: {}\n\
                 On a headless server also run: loginctl enable-linger $USER",
                String::from_utf8_lossy(&enable.stderr).trim()
            ));
        }
        Ok(unit.display().to_string())
    }

    /// Removes the registration only; the running instance is untouched
    /// (`disable` without `--now`). Returns whether a unit existed.
    pub fn unregister(exe_dir: &std::path::Path) -> Result<bool> {
        let _ = exe_dir;
        let Some(unit) = unit_path() else {
            return Ok(false);
        };
        if !unit.exists() {
            return Ok(false);
        }
        let _ = systemctl(&["disable", AUTOSTART_NAME]);
        std::fs::remove_file(&unit).with_context(|| format!("remove {}", unit.display()))?;
        let _ = systemctl(&["daemon-reload"]);
        let _ = systemctl(&["reset-failed", AUTOSTART_NAME]);
        Ok(true)
    }

    // The tray (Windows-only) is the only caller today; kept for symmetry.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn is_registered(exe_dir: &std::path::Path) -> bool {
        let _ = exe_dir;
        unit_path().map(|p| p.exists()).unwrap_or(false)
    }
}

pub use imp::{install, unregister};
// Only the tray checkbox queries the registration state.
#[cfg(windows)]
pub use imp::is_registered;

/// Temporary-stop helper for docs/prints: how to stop without unregistering.
pub fn temporary_stop_hint() -> &'static str {
    if cfg!(windows) {
        "stop.bat (or taskkill /IM readest-supervisor.exe /F) — the autostart registration stays; the next logon brings it back"
    } else {
        "systemctl --user stop readest-supervisor — the registration stays; reboot/start bring it back"
    }
}
