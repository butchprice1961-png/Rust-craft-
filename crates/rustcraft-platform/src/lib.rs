//! OS integration: single-instance lock, Java discovery, opening folders.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Held for the launcher's lifetime. The OS releases the lock if the process
/// dies, so a crash never leaves a stale lock behind.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// `Ok(None)` if another process holds the lock.
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

pub fn open_folder(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    open::that_detached(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    /// Executable to launch the game with (`javaw.exe` on Windows when present).
    pub path: PathBuf,
    pub major: u32,
    pub version: String,
    pub is_64bit: bool,
}

/// Parses `java -version` output (printed on stderr).
pub fn parse_java_version(output: &str) -> Option<(u32, String, bool)> {
    let first = output.lines().find(|l| l.contains("version \""))?;
    let version = first.split('"').nth(1)?.to_string();
    let mut parts = version.split(|c: char| !c.is_ascii_digit());
    let first_num: u32 = parts.next()?.parse().ok()?;
    let major = if first_num == 1 { parts.next()?.parse().ok()? } else { first_num };
    Some((major, version, output.contains("64-Bit")))
}

/// Runs `java -version` for one candidate executable.
pub fn probe_java(java: &Path) -> Option<JavaInstall> {
    let output = Command::new(java).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let (major, version, is_64bit) = parse_java_version(&text)?;
    let javaw = java.with_file_name("javaw.exe");
    let path = if cfg!(windows) && javaw.exists() { javaw } else { java.to_path_buf() };
    Some(JavaInstall { path, major, version, is_64bit })
}

/// Finds Java installations in the usual places. Blocking: it runs each
/// candidate once, so call it off the UI thread.
pub fn discover_java() -> Vec<JavaInstall> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(exe));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join(exe)));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let roots: Vec<(PathBuf, &str)> = if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| "C:\\Program Files".into());
        ["Java", "Eclipse Adoptium", "Microsoft", "Zulu", "BellSoft", "Amazon Corretto"]
            .into_iter()
            .map(|v| (pf.join(v), "bin"))
            .collect()
    } else if cfg!(target_os = "macos") {
        vec![
            ("/Library/Java/JavaVirtualMachines".into(), "Contents/Home/bin"),
            (home.join("Library/Java/JavaVirtualMachines"), "Contents/Home/bin"),
        ]
    } else {
        vec![
            ("/usr/lib/jvm".into(), "bin"),
            ("/usr/lib64/jvm".into(), "bin"),
            ("/opt/java".into(), "bin"),
            (home.join(".sdkman/candidates/java"), "bin"),
        ]
    };
    for (root, bin) in roots {
        if let Ok(entries) = std::fs::read_dir(&root) {
            candidates.extend(entries.flatten().map(|e| e.path().join(bin).join(exe)));
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut found: Vec<JavaInstall> = candidates
        .into_iter()
        .filter(|c| c.is_file())
        .filter(|c| seen.insert(std::fs::canonicalize(c).unwrap_or_else(|_| c.clone())))
        .filter_map(|c| probe_java(&c))
        .collect();
    found.sort_by_key(|j| std::cmp::Reverse(j.major));
    found
}

/// Best installed Java for a game that needs `major` (exact match first,
/// since old game versions break on newer Java; 64-bit preferred).
pub fn pick_java(installs: &[JavaInstall], major: u32) -> Option<&JavaInstall> {
    installs
        .iter()
        .filter(|j| j.major == major)
        .max_by_key(|j| j.is_64bit)
        .or_else(|| if major >= 17 { installs.iter().find(|j| j.major >= major) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_java_versions() {
        let j8 = "java version \"1.8.0_402\"\nJava(TM) SE Runtime Environment (build 1.8.0_402-b06)\nJava HotSpot(TM) 64-Bit Server VM";
        assert_eq!(parse_java_version(j8), Some((8, "1.8.0_402".into(), true)));
        let j21 = "openjdk version \"21.0.12\" 2026-07-15\nOpenJDK Runtime Environment\nOpenJDK 64-Bit Server VM";
        assert_eq!(parse_java_version(j21).unwrap().0, 21);
        assert_eq!(parse_java_version("openjdk version \"25-ea\"").unwrap().0, 25);
        assert_eq!(parse_java_version("Picked up JAVA_TOOL_OPTIONS: x\nopenjdk version \"17\"").unwrap().0, 17);
        assert!(parse_java_version("garbage").is_none());
    }

    #[test]
    fn picks_matching_java() {
        let j = |major, is_64bit| JavaInstall {
            path: format!("/j{major}").into(),
            major,
            version: String::new(),
            is_64bit,
        };
        let all = [j(21, true), j(17, true), j(8, false), j(8, true)];
        assert_eq!(pick_java(&all, 8).unwrap(), &all[3]);
        assert_eq!(pick_java(&all, 17).unwrap().major, 17);
        assert_eq!(pick_java(&[j(25, true)], 21).unwrap().major, 25);
        assert!(pick_java(&[j(17, true)], 8).is_none());
    }

    // A child spawned by another test holds a copy of every open file until
    // it execs, which keeps the lock alive past `drop`; keep them apart.
    static SPAWNING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn second_lock_is_refused_until_release() {
        let _guard = SPAWNING.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("launcher.lock");
        let first = InstanceLock::try_acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(InstanceLock::try_acquire(&path).unwrap().is_none());
        drop(first);
        assert!(InstanceLock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn finds_system_java_here() {
        let _guard = SPAWNING.lock().unwrap_or_else(|e| e.into_inner());
        // The dev container and CI images ship a JDK; elsewhere this only
        // checks that discovery does not panic.
        for java in discover_java() {
            assert!(java.major >= 8, "{java:?}");
        }
    }
}
