use crate::{dirent, sys::*};
use core::ptr;
const MAX_ARGS: usize = 256;
struct Args {
    values: [*const u8; MAX_ARGS],
    len: usize,
}
impl Args {
    fn new() -> Self {
        Self {
            values: [ptr::null(); MAX_ARGS],
            len: 0,
        }
    }
    fn push(&mut self, text: *const u8) -> Result<()> {
        if self.len + 1 >= MAX_ARGS {
            return Err("Too many launch arguments; limit is 255 including systemd options.");
        }
        self.values[self.len] = text;
        self.len += 1;
        self.values[self.len] = ptr::null();
        Ok(())
    }
}
unsafe fn command(
    args: &Args,
    env: *const *const u8,
    mut output: Option<&mut Buffer<8192>>,
) -> Result<i32> {
    let mut pipe = [-1i32; 2];
    if output.is_some() && call(293, pipe.as_mut_ptr() as usize, CLOEXEC, 0, 0, 0, 0) < 0 {
        return Err("Cannot open a command pipe.");
    }
    // pipe2 returned two descriptors owned by this command invocation.
    let read_end = Fd::from_raw(pipe[0]);
    let write_end = Fd::from_raw(pipe[1]);
    let child = call(57, 0, 0, 0, 0, 0, 0);
    if child < 0 {
        return Err("Cannot start a systemd command.");
    }
    if child == 0 {
        if write_end.raw() >= 0 {
            close(read_end.raw());
            if call(33, write_end.raw() as usize, 1, 0, 0, 0, 0) < 0 {
                exit(126);
            }
            close(write_end.raw());
        }
        call(
            59,
            args.values[0] as usize,
            args.values.as_ptr() as usize,
            env as usize,
            0,
            0,
            0,
        );
        error("Cannot execute the installed systemd or Steam command.");
        exit(127);
    }
    drop(write_end);
    let mut failure = None;
    if let Some(out) = output.as_mut() {
        out.clear();
        let mut chunk = [0u8; 2048];
        loop {
            let n = read(read_end.raw(), &mut chunk);
            if n < 0 {
                failure = Some("Cannot read command output.");
                break;
            }
            if n == 0 {
                break;
            }
            if out.append(&chunk[..n as usize]).is_err() {
                failure = Some("Command output exceeds the fixed buffer.");
                break;
            }
        }
    }
    if failure.is_some() {
        // The unreaped child PID cannot be reused. Stop the writer before waiting.
        while call(62, child as usize, 9, 0, 0, 0, 0) == -4 {}
    }
    drop(read_end);
    let mut status = 0i32;
    loop {
        let rc = call(
            61,
            child as usize,
            &mut status as *mut _ as usize,
            0,
            0,
            0,
            0,
        );
        if rc == -4 {
            continue;
        }
        if rc < 0 {
            return Err("Cannot wait for the launch command.");
        }
        break;
    }
    if let Some(message) = failure {
        return Err(message);
    }
    Ok(if status & 0x7f == 0 {
        (status >> 8) & 255
    } else {
        128 + (status & 0x7f)
    })
}
struct State {
    active: bool,
    closing: bool,
    loaded: bool,
    group: Buffer<4096>,
}
unsafe fn state(env: *const *const u8) -> Result<State> {
    let mut args = Args::new();
    for item in [
        b"/usr/bin/systemctl\0".as_slice(),
        b"--user\0",
        b"show\0",
        b"steam-high-priority-session.service\0",
        b"-p\0",
        b"ActiveState\0",
        b"-p\0",
        b"LoadState\0",
        b"-p\0",
        b"ControlGroup\0",
    ] {
        args.push(item.as_ptr())?;
    }
    let mut output = Buffer::<8192>::new();
    command(&args, env, Some(&mut output))?;
    let mut result = State {
        active: false,
        closing: false,
        loaded: false,
        group: Buffer::new(),
    };
    let mut have_state = false;
    for line in output.data().split(|b| *b == b'\n') {
        if let Some(value) = line.strip_prefix(b"ActiveState=") {
            have_state = true;
            result.active = matches!(value, b"active" | b"activating" | b"reloading");
            result.closing = value == b"deactivating";
        }
        if let Some(value) = line.strip_prefix(b"LoadState=") {
            result.loaded = value == b"loaded";
        }
        if let Some(value) = line.strip_prefix(b"ControlGroup=") {
            result.group.append(value)?;
        }
    }
    if !have_state {
        return Err("Cannot contact the user service manager.");
    }
    Ok(result)
}
fn home_path(home: &[u8], suffix: &[u8]) -> Result<Buffer<4096>> {
    let mut path = Buffer::new();
    path.append(home)?;
    path.append(suffix)?;
    Ok(path)
}
struct Identity {
    pid: u32,
    start: u64,
    managed: bool,
}
fn identity(home: &[u8], group: &[u8]) -> Result<Option<Identity>> {
    let pidpath = home_path(home, b"/.steam/steam.pid")?;
    let mut data = [0u8; 16384];
    let n = match read_file(AT_FDCWD, pidpath.ptr(), &mut data) {
        Ok(n) => n,
        Err(-2) => return Ok(None),
        Err(_) => return Err("Cannot read Steam's PID file."),
    };
    let pid = match decimal(trim(&data[..n])) {
        Some(p) if p > 1 && p <= i32::MAX as u64 => p as u32,
        _ => return Ok(None),
    };
    let mut path = Buffer::<64>::new();
    path.append(b"/proc/")?;
    path.number(pid as u64)?;
    let directory = match openat(AT_FDCWD, path.ptr(), RDONLY | DIRECTORY | NOFOLLOW, 0) {
        Ok(fd) => fd,
        Err(-2) => return Ok(None),
        Err(_) => return Err("Cannot inspect Steam's process."),
    };
    if stat(directory.raw() as usize, b"\0".as_ptr(), 0x1000)
        .map_err(|_| "Cannot inspect Steam's owner.")?
        .uid
        != uid()
    {
        return Ok(None);
    }
    let n = match read_file(directory.raw() as usize, b"comm\0".as_ptr(), &mut data) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    if trim(&data[..n]) != b"steam" {
        return Ok(None);
    }
    let n = unsafe {
        call(
            267,
            directory.raw() as usize,
            b"exe\0".as_ptr() as usize,
            data.as_mut_ptr() as usize,
            data.len(),
            0,
            0,
        )
    };
    if n <= 0 || !data[..n as usize].ends_with(b"/steam") {
        return Ok(None);
    }
    let n = match read_file(directory.raw() as usize, b"stat\0".as_ptr(), &mut data) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let end = data[..n]
        .iter()
        .rposition(|b| *b == b')')
        .ok_or("Invalid Steam process identity.")?;
    let mut fields = data[end + 1..n]
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty());
    if matches!(fields.next(), Some(b"Z") | Some(b"X") | None) {
        return Ok(None);
    }
    let start = fields
        .nth(18)
        .and_then(decimal)
        .ok_or("Invalid Steam process start time.")?;
    let n = match read_file(directory.raw() as usize, b"cgroup\0".as_ptr(), &mut data) {
        Ok(n) => n,
        Err(_) => return Ok(None),
    };
    let current = data[..n]
        .split(|b| *b == b'\n')
        .find_map(|s| s.strip_prefix(b"0::"))
        .ok_or("cgroup v2 is required.")?;
    Ok(Some(Identity {
        pid,
        start,
        managed: member(current, group),
    }))
}
fn endpoint(home: &[u8], steam: &Identity) -> Result<bool> {
    let path = home_path(home, b"/.steam/steam.pipe")?;
    let pipe = match stat(AT_FDCWD, path.ptr(), 0) {
        Ok(s) => s,
        Err(-2) => return Ok(false),
        Err(_) => return Err("Cannot inspect Steam's command pipe."),
    };
    if pipe.mode & 0xf000 != 0x1000 || pipe.uid != uid() {
        return Ok(false);
    }
    let mut path = Buffer::<80>::new();
    path.append(b"/proc/")?;
    path.number(steam.pid as u64)?;
    path.append(b"/fd")?;
    let dir = match openat(AT_FDCWD, path.ptr(), RDONLY | DIRECTORY | NOFOLLOW, 0) {
        Ok(fd) => fd,
        Err(_) => return Ok(false),
    };
    let mut data = [0u8; 4096];
    loop {
        let n = match directory_entries(dir.raw(), &mut data) {
            Ok(0) | Err(_) => return Ok(false),
            Ok(n) => n,
        };
        let mut offset = 0;
        while offset < n {
            let entry = dirent::next(&data[..n], &mut offset)?;
            if !matches!(entry.name, b".\0" | b"..\0") {
                if let Ok(info) = stat(dir.raw() as usize, entry.name.as_ptr(), 0) {
                    if same(&pipe, &info) {
                        return Ok(true);
                    }
                }
            }
        }
    }
}
fn group_alive(group: &[u8]) -> Result<bool> {
    if group.is_empty() {
        return Ok(false);
    }
    let mut path = Buffer::<4160>::new();
    path.append(b"/sys/fs/cgroup")?;
    path.append(group)?;
    path.append(b"/cgroup.events")?;
    let mut data = [0u8; 512];
    let n = match read_file(AT_FDCWD, path.ptr(), &mut data) {
        Ok(n) => n,
        Err(-2) => return Ok(false),
        Err(_) => return Err("Cannot read Steam's cgroup state."),
    };
    Ok(data[..n]
        .split(|b| *b == b'\n')
        .any(|s| s == b"populated 1"))
}
fn ready(home: &[u8], group: &[u8]) -> Result<Identity> {
    let deadline = now() + 90_000_000_000;
    loop {
        if !group_alive(group)? {
            return Err("Steam stopped during startup. Check journalctl --user -u steam-high-priority-session.service.");
        }
        if let Some(steam) = identity(home, group)? {
            if !steam.managed {
                return Err(
                    "Steam is running outside the managed group. Close Steam and open it again.",
                );
            }
            if endpoint(home, &steam)? {
                return Ok(steam);
            }
        }
        if now() >= deadline {
            return Err("Steam's command interface is not ready. Check the Steam window.");
        }
        sleep_until(now() + 200_000_000);
    }
}
fn selected_environment(name: &[u8]) -> bool {
    name.starts_with(b"LC_")
        || matches!(
            name,
            b"DISPLAY"
                | b"XAUTHORITY"
                | b"WAYLAND_DISPLAY"
                | b"XDG_SESSION_TYPE"
                | b"XDG_CURRENT_DESKTOP"
                | b"DESKTOP_SESSION"
                | b"XDG_RUNTIME_DIR"
                | b"DBUS_SESSION_BUS_ADDRESS"
                | b"PATH"
                | b"LANG"
                | b"LANGUAGE"
                | b"XDG_CONFIG_HOME"
                | b"XDG_DATA_HOME"
                | b"XDG_CACHE_HOME"
                | b"XDG_DATA_DIRS"
                | b"XDG_CONFIG_DIRS"
        )
}
pub unsafe fn run(argc: usize, argv: *const *const u8, env: *const *const u8) -> Result<i32> {
    if uid() == 0 {
        return Err("Run Steam as your normal user, not through sudo.");
    }
    let home = environment(env, b"HOME").ok_or("HOME is not set.")?;
    if home.first() != Some(&b'/') {
        return Err("HOME must be an absolute path.");
    }
    let mut runtime = Buffer::<80>::new();
    runtime.append(b"/run/user/")?;
    runtime.number(uid() as u64)?;
    if stat(AT_FDCWD, runtime.ptr(), 0)
        .map_err(|_| "Cannot inspect the user runtime directory.")?
        .uid
        != uid()
    {
        return Err("Invalid user runtime directory owner.");
    }
    runtime.append(b"/steam-high-priority.lock")?;
    let lock = openat(AT_FDCWD, runtime.ptr(), 2 | 64 | NOFOLLOW, 0o600)
        .map_err(|_| "Cannot open the Steam launch lock.")?;
    let info = stat(lock.raw() as usize, b"\0".as_ptr(), 0x1000)
        .map_err(|_| "Cannot inspect the Steam launch lock.")?;
    if info.uid != uid() || info.mode & 0xf000 != 0x8000 {
        return Err("Invalid Steam launch lock.");
    }
    loop {
        let rc = call(73, lock.raw() as usize, 2, 0, 0, 0, 0);
        if rc == -4 {
            continue;
        }
        if rc < 0 {
            return Err("Cannot lock the Steam launcher.");
        }
        break;
    }
    let mut status = state(env)?;
    if let Some(steam) = identity(home, status.group.data())? {
        if !steam.managed {
            return Err(
                "Steam is running outside the managed group. Close Steam and open it again.",
            );
        }
    }
    if status.closing {
        return Err("Steam is closing. Open it again after shutdown completes.");
    }
    if status.active {
        let steam = ready(home, status.group.data())?;
        let remote = home_path(
            home,
            b"/.steam/root/ubuntu12_32/steam-runtime/amd64/usr/bin/steam-runtime-steam-remote",
        )?;
        let mut args = Args::new();
        args.push(remote.ptr())?;
        for i in 1..argc {
            args.push(*argv.add(i))?;
        }
        let fresh = identity(home, status.group.data())?
            .ok_or("Steam exited before forwarding the request.")?;
        if fresh.pid != steam.pid || fresh.start != steam.start || !fresh.managed {
            return Err("Steam changed during the request. Open Steam again.");
        }
        return command(&args, env, None);
    }
    if status.loaded {
        let mut reset = Args::new();
        for arg in [
            b"/usr/bin/systemctl\0".as_slice(),
            b"--user\0",
            b"reset-failed\0",
            b"steam-high-priority-session.service\0",
        ] {
            reset.push(arg.as_ptr())?;
        }
        command(&reset, env, None)?;
        for _ in 0..20 {
            status = state(env)?;
            if !status.loaded {
                break;
            }
            sleep_until(now() + 100_000_000);
        }
        if status.loaded {
            return Err("The previous Steam unit has not finished unloading.");
        }
    }
    let mut args = Args::new();
    for arg in [
        b"/usr/bin/systemd-run\0".as_slice(),
        b"--user\0",
        b"--quiet\0",
        b"--collect\0",
        b"--expand-environment=no\0",
        b"--unit=steam-high-priority-session.service\0",
        b"--service-type=exec\0",
        b"--property=Description=Managed Steam session\0",
        b"--property=Slice=app.slice\0",
        b"--property=ExitType=cgroup\0",
        b"--property=Nice=0\0",
        b"--property=LimitNICE=-19\0",
        b"--property=Restart=no\0",
        b"--property=KillMode=control-group\0",
    ] {
        args.push(arg.as_ptr())?;
    }
    let mut cwd = [0u8; 4096];
    let n = call(79, cwd.as_mut_ptr() as usize, cwd.len(), 0, 0, 0, 0);
    if n < 1 {
        return Err("Cannot read the working directory.");
    }
    let mut directory = Buffer::<4160>::new();
    directory.append(b"--working-directory=")?;
    directory.append(&cwd[..n as usize - 1])?;
    args.push(directory.ptr())?;
    let mut environment_args = [0u8; 8192];
    let mut used = 0;
    let mut i = 0;
    while !(*env.add(i)).is_null() {
        let item = cstr(*env.add(i));
        i += 1;
        let Some(end) = item.iter().position(|b| *b == b'=') else {
            continue;
        };
        let name = &item[..end];
        if !selected_environment(name) {
            continue;
        }
        let count = 9 + name.len() + 1;
        if used + count > environment_args.len() {
            return Err("Environment names exceed the fixed launch buffer.");
        }
        environment_args[used..used + 9].copy_from_slice(b"--setenv=");
        environment_args[used + 9..used + 9 + name.len()].copy_from_slice(name);
        args.push(environment_args.as_ptr().add(used))?;
        used += count;
    }
    let mut steam_path = Buffer::<4096>::new();
    let n = read_file(
        AT_FDCWD,
        b"/usr/local/libexec/steam-high-priority/steam-path\0".as_ptr(),
        &mut steam_path.bytes,
    )
    .map_err(|_| "Cannot read the installed Steam path. Run the installer again.")?;
    let value = trim(&steam_path.bytes[..n]);
    if !matches!(value, b"/usr/bin/steam" | b"/usr/games/steam") {
        return Err("The installed Steam path is invalid. Run the installer again.");
    }
    let len = value.len();
    if steam_path.bytes[0] != b'/' {
        return Err("Invalid Steam path.");
    }
    steam_path.bytes[len] = 0;
    steam_path.len = len;
    args.push(b"--\0".as_ptr())?;
    args.push(steam_path.ptr())?;
    for i in 1..argc {
        args.push(*argv.add(i))?;
    }
    let rc = command(&args, env, None)?;
    if rc != 0 {
        return Err("The managed Steam service could not start.");
    }
    // A new session can overlap the previous poller's empty-group exit.
    // Restart exactly once here; requests forwarded to a live session do not restart it.
    let mut policy = Args::new();
    for arg in [
        b"/usr/bin/systemctl\0".as_slice(),
        b"--user\0",
        b"restart\0",
        b"steam-high-priority.service\0",
    ] {
        policy.push(arg.as_ptr())?;
    }
    if command(&policy, env, None)? != 0 {
        return Err(
            "Steam started, but its priority service could not start. Check the user journal.",
        );
    }
    status = state(env)?;
    ready(home, status.group.data())?;
    Ok(0)
}
