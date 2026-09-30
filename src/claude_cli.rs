//! Locates the Claude Code executable portably.
//!
//! On Windows, `Command::new("claude")` only tries `claude.exe`, but an npm
//! install creates `claude.cmd` (next to an extensionless `claude` script that
//! Windows can't run). So we search `PATH` trying each `PATHEXT` extension and
//! launch the full path; since Rust 1.77 the standard library runs `.cmd`/`.bat`
//! through `cmd.exe` with safely escaped arguments.

use std::{
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::Command,
    sync::OnceLock,
};

const PROGRAM: &str = "claude";

/// A `Command` ready to run Claude Code.
///
/// If it isn't found on `PATH`, the bare name is used so that spawning fails
/// with the usual "not found" error.
pub fn command() -> Command {
    static RESOLVED: OnceLock<Option<PathBuf>> = OnceLock::new();
    let resolved = RESOLVED.get_or_init(|| {
        let path = env::var_os("PATH")?;
        find_executable(PROGRAM, &path, &executable_extensions())
    });
    match resolved {
        Some(path) => Command::new(path),
        None => Command::new(PROGRAM),
    }
}

/// Extensions to try: those in `PATHEXT` on Windows; none elsewhere.
fn executable_extensions() -> Vec<OsString> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    pathext
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(OsString::from)
        .collect()
}

/// Searches each directory in `path`, in order, for `name`.
///
/// With empty `extensions` (Unix) it looks for the exact name; otherwise it
/// tries `name` + each extension and never the bare name.
fn find_executable(name: &str, path: &OsStr, extensions: &[OsString]) -> Option<PathBuf> {
    env::split_paths(path).find_map(|dir| {
        if extensions.is_empty() {
            let candidate = dir.join(name);
            return candidate.is_file().then_some(candidate);
        }
        extensions.iter().find_map(|ext| {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(file);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("claudash-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prefers_extension_over_bare_npm_shim() {
        let dir = temp_dir("pathext");
        // npm creates all three; the extensionless script can't run on Windows.
        for file in ["claude", "claude.cmd", "claude.ps1"] {
            fs::write(dir.join(file), "").unwrap();
        }
        let path = env::join_paths([&dir]).unwrap();
        // Lowercase: Windows file names are case-insensitive, Linux ones are not.
        let exts: Vec<OsString> = [".exe", ".cmd"].map(OsString::from).to_vec();

        let found = find_executable("claude", &path, &exts);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, Some(dir.join("claude.cmd")));
    }

    #[test]
    fn respects_path_order_and_missing_dirs() {
        let first = temp_dir("order-a");
        let second = temp_dir("order-b");
        fs::write(second.join("claude"), "").unwrap();
        let path = env::join_paths([first.join("missing"), first.clone(), second.clone()]).unwrap();

        let found = find_executable("claude", &path, &[]);
        let missing = find_executable("otro", &path, &[]);
        fs::remove_dir_all(&first).unwrap();
        fs::remove_dir_all(&second).unwrap();
        assert_eq!(found, Some(second.join("claude")));
        assert_eq!(missing, None);
    }

    /// Actually runs a fake `claude.cmd`: checks that Rust launches it through
    /// `cmd.exe` and that arguments arrive intact. Windows only (runs in CI).
    #[cfg(windows)]
    #[test]
    fn runs_cmd_shim_with_arguments() {
        let dir = temp_dir("cmd-shim");
        fs::write(dir.join("claude.cmd"), "@echo off\r\necho args=%1 %2\r\n").unwrap();
        let path = env::join_paths([&dir]).unwrap();
        let exe = find_executable("claude", &path, &executable_extensions()).unwrap();

        let output = Command::new(&exe)
            .args(["--resume", "0fb00c9a-2bf4-4096-ba7e-c46880a43b21"])
            .output()
            .unwrap();
        fs::remove_dir_all(&dir).unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            stdout.trim(),
            "args=--resume 0fb00c9a-2bf4-4096-ba7e-c46880a43b21"
        );
    }
}
