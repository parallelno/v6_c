use std::process::Command;

fn main() {
    // Build date: YYYY.MM.DD
    let date = chrono_free_date();
    println!("cargo:rustc-env=V6C_BUILD_DATE={}", date);

    // Git short hash
    let hash = git_short_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=V6C_BUILD_HASH={}", hash);

    // Rebuild if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
}

fn chrono_free_date() -> String {
    // Try `date` command on Unix or use PowerShell on Windows
    if cfg!(target_os = "windows") {
        Command::new("powershell")
            .args(["-NoProfile", "-Command", "Get-Date -Format 'yyyy.MM.dd'"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        Command::new("date")
            .arg("+%Y.%m.%d")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_string())
    }
}

fn git_short_hash() -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}
