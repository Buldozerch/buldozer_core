use std::ffi::OsString;
use std::io;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub branch: String,
    pub local_hash: String,
    pub remote_hash: String,
    pub remote_subject: String,
    pub behind: u32,
    pub ahead: u32,
    pub ff_possible: bool,
}

pub fn check_update() -> io::Result<Option<UpdateInfo>> {
    if !std::path::Path::new(".git").exists() {
        return Ok(None);
    }

    // Fetch remote refs.
    let _ = git(["fetch", "--quiet", "origin"])?;

    let branch = git(["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    if branch.is_empty() || branch == "HEAD" {
        return Ok(None);
    }

    let remote_ref = format!("origin/{branch}");
    let local_hash = git(["rev-parse", "--short=7", "HEAD"])?.trim().to_string();
    let remote_hash = git(["rev-parse", "--short=7", &remote_ref])?
        .trim()
        .to_string();
    if local_hash.is_empty() || remote_hash.is_empty() {
        return Ok(None);
    }

    let remote_subject = git(["log", "-1", "--format=%s", &remote_ref])?
        .trim()
        .to_string();

    // rev-list --left-right --count HEAD...origin/branch -> "<ahead>\t<behind>"
    let counts = git([
        "rev-list",
        "--left-right",
        "--count",
        &format!("HEAD...{remote_ref}"),
    ])?;
    let mut it = counts.split_whitespace();
    let ahead: u32 = it.next().unwrap_or("0").parse().unwrap_or(0);
    let behind: u32 = it.next().unwrap_or("0").parse().unwrap_or(0);

    // Only prompt for updates when we're behind the remote.
    if behind == 0 {
        return Ok(None);
    }

    let ff_possible = ahead == 0;

    Ok(Some(UpdateInfo {
        branch,
        local_hash,
        remote_hash,
        remote_subject,
        behind,
        ahead,
        ff_possible,
    }))
}

pub fn pull_ff_only() -> io::Result<()> {
    // Safety: avoid merges.
    let dirty = !git(["status", "--porcelain"])?.trim().is_empty();
    if dirty {
        return Err(io::Error::other(
            "working tree has local changes; commit/stash before update",
        ));
    }

    let _ = git(["pull", "--ff-only", "--quiet"])?;
    Ok(())
}

pub fn restart_self() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    Command::new(exe).args(args).spawn().map(|_| ())
}

fn git<I, S>(args: I) -> io::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    // Avoid blocking credential prompts inside TUI.
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
