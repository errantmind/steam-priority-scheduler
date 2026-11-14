use crate::sys::*;
const DEPTH: usize = 32;
#[derive(Clone, Copy)]
struct Level {
    fd: i32,
    pos: usize,
    end: usize,
    data: [u8; 4096],
}
impl Level {
    const ZERO: Self = Self {
        fd: 0,
        pos: 0,
        end: 0,
        data: [0; 4096],
    };
}

// Only these descriptors persist between passes. Nested groups and TIDs stay fresh.
struct Cache {
    directory: Fd,
    events: Fd,
}
impl Cache {
    fn new() -> Self {
        Self {
            directory: Fd(-1),
            events: Fd(-1),
        }
    }
    fn clear(&mut self) {
        self.events = Fd(-1);
        self.directory = Fd(-1);
    }
    fn open(&mut self, path: *const u8) -> Result<bool> {
        if self.directory.0 >= 0 && self.events.0 >= 0 {
            return Ok(true);
        }
        self.clear();
        let directory = match openat(AT_FDCWD, path, RDONLY | DIRECTORY | NOFOLLOW, 0) {
            Ok(fd) => fd,
            Err(-2) => return Ok(false),
            Err(_) => return Err("Cannot open Steam's cgroup; retrying on the next pass."),
        };
        let events = openat(
            directory.0 as usize,
            b"cgroup.events\0".as_ptr(),
            RDONLY | NOFOLLOW,
            0,
        )
        .map_err(|_| "Cannot open Steam's cgroup state; retrying on the next pass.")?;
        self.directory = directory;
        self.events = events;
        Ok(true)
    }
    fn populated(&self) -> Result<bool> {
        rewind(self.events.0).map_err(|_| "Cannot rewind Steam's cached state file.")?;
        let mut input = [0u8; 512];
        let n = read_fd(self.events.0, &mut input)
            .map_err(|_| "Cannot read Steam's cached state file.")?;
        for line in input[..n].split(|b| *b == b'\n') {
            if line == b"populated 1" {
                return Ok(true);
            }
            if line == b"populated 0" {
                return Ok(false);
            }
        }
        Err("Steam's cgroup has no population field.")
    }
    fn empty_is_current(&self, path: *const u8) -> Result<bool> {
        // An empty cached group may have been replaced at the same path.
        let current = match stat(AT_FDCWD, path, 0x100) {
            Ok(info) => info,
            Err(-2) => return Ok(true),
            Err(_) => return Err("Cannot check the current Steam group."),
        };
        let cached = stat(self.directory.0 as usize, b"\0".as_ptr(), 0x1000)
            .map_err(|_| "Cannot check the cached Steam group.")?;
        Ok(same(&current, &cached))
    }
    fn pass(&mut self, path: *const u8, mut work: impl FnMut(i32) -> Result<()>) -> Result<bool> {
        let mut failure = "Cannot read Steam's cgroup.";
        // Initial attempt plus one recovery attempt. Further work waits for the next deadline.
        for _ in 0..2 {
            let result = (|| {
                if !self.open(path)? {
                    return Ok(false);
                }
                if !self.populated()? {
                    if self.empty_is_current(path)? {
                        return Ok(false);
                    }
                    return Err("Steam's cgroup was replaced.");
                }
                rewind(self.directory.0).map_err(|_| "Cannot rewind Steam's cached directory.")?;
                work(self.directory.0)?;
                Ok(true)
            })();
            match result {
                Ok(alive) => {
                    if !alive {
                        self.clear();
                    }
                    return Ok(alive);
                }
                Err(message) => {
                    failure = message;
                    self.clear();
                }
            }
        }
        Err(failure)
    }
}

struct Notice {
    failed: bool,
}
impl Notice {
    fn update(&mut self, issue: Option<&'static str>) {
        match (self.failed, issue) {
            (false, Some(message)) => {
                error(message);
                self.failed = true;
            }
            (true, None) => {
                put(2, b"Steam priority: polling recovered.\n");
                self.failed = false;
            }
            _ => {}
        }
    }
}

fn thread_group(directory: i32, data: &mut [u8; 16384], root: &[u8]) -> Result<bool> {
    let n = match read_file(directory as usize, b"cgroup\0".as_ptr(), data) {
        Ok(n) => n,
        Err(-2) | Err(-3) => return Ok(false),
        Err(_) => return Err("Cannot read a thread cgroup."),
    };
    Ok(data[..n]
        .split(|b| *b == b'\n')
        .any(|s| s.starts_with(b"0::") && member(&s[3..], root)))
}
fn correct(tid: u32, root: &[u8], scratch: &mut [u8; 16384]) -> Result<()> {
    // The raw getpriority syscall returns 20 - nice, so nice -19 is 39.
    let priority = unsafe { call(140, 0, tid as usize, 0, 0, 0, 0) };
    if priority == 39 || priority == -3 {
        return Ok(());
    }
    if priority < 0 {
        return Err("Cannot read a thread priority.");
    }
    let mut path = Buffer::<64>::new();
    path.append(b"/proc/")?;
    path.number(tid as u64)?;
    let directory = match openat(AT_FDCWD, path.ptr(), RDONLY | DIRECTORY | NOFOLLOW, 0) {
        Ok(fd) => fd,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot open a Steam thread."),
    };
    let original = match stat(directory.0 as usize, b"\0".as_ptr(), 0x1000) {
        Ok(s) => s,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot inspect a Steam thread."),
    };
    if original.uid != uid() || !thread_group(directory.0, scratch, root)? {
        return Ok(());
    }
    let current = match stat(AT_FDCWD, path.ptr(), 0x100) {
        Ok(s) => s,
        Err(-2) | Err(-3) => return Ok(()),
        Err(_) => return Err("Cannot recheck a Steam thread identity."),
    };
    if !same(&original, &current) {
        return Ok(());
    }
    let rc = unsafe { call(141, 0, tid as usize, (-19isize) as usize, 0, 0, 0) };
    if rc == 0 || rc == -3 {
        Ok(())
    } else {
        Err("Cannot set a Steam thread to -19. Check LimitNICE.")
    }
}
fn correct_file(
    directory: i32,
    root: &[u8],
    input: &mut [u8; 8192],
    scratch: &mut [u8; 16384],
    issue: &mut Option<&'static str>,
) -> Result<()> {
    let fd = match openat(
        directory as usize,
        b"cgroup.threads\0".as_ptr(),
        RDONLY | NOFOLLOW,
        0,
    ) {
        Ok(fd) => fd,
        Err(-2) => return Ok(()),
        Err(_) => return Err("Cannot read Steam's thread list."),
    };
    let mut value = 0u64;
    let mut digits = false;
    loop {
        let n = read(fd.0, input);
        if n < 0 {
            return Err("Cannot read Steam's thread list.");
        }
        if n == 0 {
            break;
        }
        for &b in &input[..n as usize] {
            if b.is_ascii_digit() {
                value = value
                    .checked_mul(10)
                    .and_then(|v| v.checked_add((b - b'0') as u64))
                    .ok_or("Invalid thread ID.")?;
                if value > i32::MAX as u64 {
                    return Err("Invalid thread ID.");
                }
                digits = true;
            } else if b.is_ascii_whitespace() {
                if digits && value > 0 {
                    if let Err(message) = correct(value as u32, root, scratch) {
                        issue.get_or_insert(message);
                    }
                }
                digits = false;
                value = 0;
            } else {
                return Err("Invalid thread list.");
            }
        }
    }
    if digits && value > 0 {
        if let Err(message) = correct(value as u32, root, scratch) {
            issue.get_or_insert(message);
        }
    }
    Ok(())
}
fn scan(
    directory: i32,
    root: &[u8],
    levels: &mut [Level; DEPTH],
    input: &mut [u8; 8192],
    scratch: &mut [u8; 16384],
    issue: &mut Option<&'static str>,
) -> Result<()> {
    // Level zero borrows the cached descriptor. The scan owns only nested descriptors.
    levels[0].fd = directory;
    levels[0].pos = 0;
    levels[0].end = 0;
    let result = (|| {
        let mut depth = 0;
        correct_file(levels[0].fd, root, input, scratch, issue)?;
        loop {
            let level = &mut levels[depth];
            if level.pos == level.end {
                let n = unsafe {
                    call(
                        217,
                        level.fd as usize,
                        level.data.as_mut_ptr() as usize,
                        level.data.len(),
                        0,
                        0,
                        0,
                    )
                };
                if n == -4 {
                    continue;
                }
                if n < 0 {
                    return Err("Cannot enumerate Steam's nested cgroups.");
                }
                if n == 0 {
                    if depth == 0 {
                        break;
                    }
                    close(level.fd);
                    level.fd = -1;
                    depth -= 1;
                    continue;
                }
                level.pos = 0;
                level.end = n as usize;
            }
            let offset = level.pos;
            if offset + 19 > level.end {
                return Err("Invalid directory record.");
            }
            let length =
                u16::from_ne_bytes([level.data[offset + 16], level.data[offset + 17]]) as usize;
            if length < 20 || offset + length > level.end {
                return Err("Invalid directory record.");
            }
            level.pos += length;
            let dtype = level.data[offset + 18];
            if dtype != 4 && dtype != 0 {
                continue;
            }
            let tail = &level.data[offset + 19..offset + length];
            let name_end = tail
                .iter()
                .position(|b| *b == 0)
                .ok_or("Invalid directory name.")?;
            let name = &tail[..name_end];
            if name == b"." || name == b".." {
                continue;
            }
            let child = match openat(
                level.fd as usize,
                tail.as_ptr(),
                RDONLY | DIRECTORY | NOFOLLOW,
                0,
            ) {
                Ok(fd) => fd,
                Err(-2) | Err(-20) | Err(-40) => continue,
                Err(_) => return Err("Cannot open a nested Steam cgroup."),
            };
            if depth + 1 >= DEPTH {
                return Err("Steam's cgroup depth exceeds the fixed limit of 32.");
            }
            depth += 1;
            levels[depth].fd = child.0;
            core::mem::forget(child);
            levels[depth].pos = 0;
            levels[depth].end = 0;
            correct_file(levels[depth].fd, root, input, scratch, issue)?;
        }
        Ok(())
    })();
    levels[0].fd = -1;
    for level in &mut levels[1..] {
        close(level.fd);
        level.fd = -1;
    }
    result
}
pub fn run() -> Result<i32> {
    let mut scratch = [0u8; 16384];
    let n = read_file(AT_FDCWD, b"/proc/self/cgroup\0".as_ptr(), &mut scratch)
        .map_err(|_| "Cannot read the policy cgroup.")?;
    let own = scratch[..n]
        .split(|b| *b == b'\n')
        .find(|s| s.starts_with(b"0::"))
        .ok_or("cgroup v2 is required.")?;
    let own = &own[3..];
    let suffix = b"/steam-high-priority.service";
    if !own.ends_with(suffix) {
        return Err("Run this poller through steam-high-priority.service.");
    }
    let mut group = Buffer::<4096>::new();
    group.append(&own[..own.len() - suffix.len()])?;
    group.append(b"/steam-high-priority-session.service")?;
    let mut path = Buffer::<4096>::new();
    path.append(b"/sys/fs/cgroup")?;
    path.append(group.data())?;
    // Fixed storage is initialized before the first pass and reused thereafter.
    let mut levels = [Level::ZERO; DEPTH];
    // Set every descriptor to the unused value before scanning or cleanup.
    for level in &mut levels {
        level.fd = -1;
    }
    let mut input = [0u8; 8192];
    put(2, b"Steam priority: native Rust, nice=-19, interval=2s.\n");
    let mut deadline = now();
    let mut cache = Cache::new();
    let mut notice = Notice { failed: false };
    loop {
        let mut issue = None;
        match cache.pass(path.ptr(), |directory| {
            scan(
                directory,
                group.data(),
                &mut levels,
                &mut input,
                &mut scratch,
                &mut issue,
            )
        }) {
            Ok(false) => break,
            Ok(true) => notice.update(issue),
            Err(message) => notice.update(Some(message)),
        }
        let current = now();
        loop {
            deadline += 2_000_000_000;
            if deadline > current {
                break;
            }
        }
        sleep_until(deadline);
    }
    Ok(0)
}
