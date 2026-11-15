#![forbid(unsafe_code)]

use crate::{dirent, sys::*};

/// Steam's native launch wrapper creates these sibling application scopes.
pub fn scope_name(name: &[u8]) -> bool {
    let Some(body) = name
        .strip_prefix(b"app-steam-app")
        .and_then(|s| s.strip_suffix(b".scope"))
    else {
        return false;
    };
    let Some(split) = body.iter().position(|&b| b == b'-') else {
        return false;
    };
    matches!(decimal(&body[..split]), Some(1..=u64::MAX))
        && matches!(decimal(&body[split + 1..]), Some(1..=2147483647))
}
fn client_executable(path: &[u8]) -> bool {
    let path = path.strip_suffix(b" (deleted)").unwrap_or(path);
    matches!(
        path.rsplit(|b| *b == b'/').next(),
        Some(b"steam") | Some(b"steamwebhelper")
    )
}
fn marker(record: &[u8]) -> bool {
    [
        b"SteamAppId=".as_slice(),
        b"SteamGameId=",
        b"STEAM_COMPAT_APP_ID=",
    ]
    .iter()
    .any(|key| {
        record
            .strip_prefix(*key)
            .and_then(decimal)
            .is_some_and(|v| v != 0)
    })
}
// Retain only one short environment record. Larger records cannot be numeric app markers.
struct Marker {
    record: [u8; 64],
    used: usize,
}
impl Marker {
    fn new() -> Self {
        Self {
            record: [0; 64],
            used: 0,
        }
    }
    fn feed(&mut self, data: &[u8]) -> bool {
        for &b in data {
            if b == 0 {
                if self.used <= self.record.len() && marker(&self.record[..self.used]) {
                    return true;
                }
                self.used = 0;
            } else if self.used < self.record.len() {
                self.record[self.used] = b;
                self.used += 1;
            } else {
                self.used = self.record.len() + 1;
            }
        }
        false
    }
}

/// Identify fallback games once per process, then visit their current threads.
/// task_data borrows a traversal buffer before that level's cgroup enumeration begins.
pub fn threads(
    pid: Tid,
    task_data: &mut [u8],
    scratch: &mut [u8; 16384],
    mut visit: impl FnMut(Tid, &mut [u8; 16384]),
) -> Result<()> {
    let mut path = Buffer::<64>::new();
    path.append(b"/proc/")?;
    path.number(pid.get() as u64)?;
    let directory = match openat(AT_FDCWD, path.ptr(), RDONLY | DIRECTORY | NOFOLLOW, 0) {
        Ok(fd) => fd,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot inspect a fallback game process."),
    };
    let info = stat(directory.raw() as usize, b"\0".as_ptr(), 0x1000)
        .map_err(|_| "Cannot check a fallback process owner.")?;
    if info.uid != uid() {
        return Ok(());
    }
    let n = match read_link(directory.raw(), b"exe\0", scratch) {
        Ok(n) => n,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot read a fallback process executable."),
    };
    if client_executable(&scratch[..n]) {
        return Ok(());
    }
    let environment = match openat(
        directory.raw() as usize,
        b"environ\0".as_ptr(),
        RDONLY | NOFOLLOW,
        0,
    ) {
        Ok(fd) => fd,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot read fallback application markers."),
    };
    let mut parser = Marker::new();
    loop {
        let n = read(environment.raw(), scratch);
        if n == 0 || n == -2 || n == -3 {
            return Ok(());
        }
        if n < 0 {
            return Err("Cannot read fallback application markers.");
        }
        if parser.feed(&scratch[..n as usize]) {
            break;
        }
    }
    drop(environment);
    let tasks = match openat(
        directory.raw() as usize,
        b"task\0".as_ptr(),
        RDONLY | DIRECTORY | NOFOLLOW,
        0,
    ) {
        Ok(fd) => fd,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot list fallback game threads."),
    };
    loop {
        let n = match directory_entries(tasks.raw(), task_data) {
            Ok(n) => n,
            Err(-2) | Err(-3) => return Ok(()),
            Err(_) => return Err("Cannot enumerate fallback game threads."),
        };
        if n == 0 {
            return Ok(());
        }
        let mut cursor = 0;
        while cursor < n {
            let entry = dirent::next(&task_data[..n], &mut cursor)?;
            if let Some(value) = decimal(&entry.name[..entry.name.len() - 1]) {
                if let Ok(value) = u32::try_from(value) {
                    if let Some(tid) = Tid::new(value) {
                        visit(tid, scratch);
                    }
                }
            }
        }
    }
}
