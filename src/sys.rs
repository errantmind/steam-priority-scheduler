// Native Linux x86-64 ABI. No libc or allocator.
use core::arch::asm;
use core::fmt::{self, Write};

pub const AT_FDCWD: usize = (-100isize) as usize;
pub const RDONLY: usize = 0;
pub const CLOEXEC: usize = 0x80000;
pub const DIRECTORY: usize = 0x10000;
pub const NOFOLLOW: usize = 0x20000;
pub type Result<T> = core::result::Result<T, &'static str>;

#[inline]
pub unsafe fn call(n: usize, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> isize {
    let result: isize;
    asm!("syscall", inlateout("rax") n as isize => result,
         in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
         in("r8") e, in("r9") f, lateout("rcx") _, lateout("r11") _, options(nostack));
    result
}

pub fn exit(code: i32) -> ! {
    unsafe {
        call(231, code as usize, 0, 0, 0, 0, 0);
    }
    loop {}
}
pub fn uid() -> u32 {
    unsafe { call(102, 0, 0, 0, 0, 0, 0) as u32 }
}
pub fn close(fd: i32) {
    if fd >= 0 {
        unsafe {
            call(3, fd as usize, 0, 0, 0, 0, 0);
        }
    }
}
pub fn put(fd: i32, bytes: &[u8]) {
    let mut offset = 0;
    while offset < bytes.len() {
        let n = unsafe {
            call(
                1,
                fd as usize,
                bytes[offset..].as_ptr() as usize,
                bytes.len() - offset,
                0,
                0,
                0,
            )
        };
        if n == -4 {
            continue;
        }
        if n <= 0 {
            break;
        }
        offset += n as usize;
    }
}
pub fn error(message: &str) {
    put(2, b"Steam policy: ");
    put(2, message.as_bytes());
    put(2, b"\n");
}

#[repr(transparent)]
pub struct Fd(i32);
impl Fd {
    pub const fn invalid() -> Self {
        Self(-1)
    }
    /// Take ownership of a newly returned descriptor, or the unused value -1.
    ///
    /// # Safety
    /// A live descriptor must have no other owner responsible for closing it.
    pub unsafe fn from_raw(fd: i32) -> Self {
        Self(fd)
    }
    pub fn raw(&self) -> i32 {
        self.0
    }
    /// Transfer ownership to the caller, which must close the descriptor once.
    pub fn into_raw(self) -> i32 {
        let fd = self.0;
        core::mem::forget(self);
        fd
    }
}
impl Drop for Fd {
    fn drop(&mut self) {
        close(self.0);
    }
}
pub fn openat(
    directory: usize,
    path: *const u8,
    flags: usize,
    mode: usize,
) -> core::result::Result<Fd, isize> {
    loop {
        let n = unsafe { call(257, directory, path as usize, flags | CLOEXEC, mode, 0, 0) };
        if n == -4 {
            continue;
        }
        return if n < 0 { Err(n) } else { Ok(Fd(n as i32)) };
    }
}
/// Linux TID accepted by priority syscalls; zero would select the caller.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Tid(u32);
impl Tid {
    pub fn new(value: u32) -> Option<Self> {
        (value > 0 && value <= i32::MAX as u32).then_some(Self(value))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

/// Raw Linux priority is 20 - nice: nice -19 is 39. Errors are negative errno.
pub fn get_priority(tid: Tid) -> isize {
    // SAFETY: getpriority takes scalar arguments and writes no user memory.
    unsafe { call(140, 0, tid.get() as usize, 0, 0, 0, 0) }
}
pub fn set_high_priority(tid: Tid) -> isize {
    // SAFETY: setpriority takes scalar arguments and writes no user memory.
    unsafe { call(141, 0, tid.get() as usize, (-19isize) as usize, 0, 0, 0) }
}

/// Fill a borrowed buffer with Linux directory records. Retry interrupted reads.
pub fn directory_entries(fd: i32, buffer: &mut [u8]) -> core::result::Result<usize, isize> {
    loop {
        // SAFETY: the kernel receives an exclusive writable slice for this call.
        // It does not retain the pointer and can write at most buffer.len() bytes.
        let n = unsafe {
            call(
                217,
                fd as usize,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            )
        };
        if n == -4 {
            continue;
        }
        return if n < 0 {
            Err(n)
        } else if n as usize > buffer.len() {
            Err(-75)
        } else {
            Ok(n as usize)
        };
    }
}

pub fn rewind(fd: i32) -> core::result::Result<(), isize> {
    loop {
        let n = unsafe { call(8, fd as usize, 0, 0, 0, 0, 0) };
        if n != -4 {
            return if n < 0 { Err(n) } else { Ok(()) };
        }
    }
}
pub fn read(fd: i32, buffer: &mut [u8]) -> isize {
    loop {
        let n = unsafe {
            call(
                0,
                fd as usize,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            )
        };
        if n != -4 {
            return n;
        }
    }
}
pub fn read_file(
    directory: usize,
    path: *const u8,
    buffer: &mut [u8],
) -> core::result::Result<usize, isize> {
    let fd = openat(directory, path, RDONLY | NOFOLLOW, 0)?;
    read_fd(fd.0, buffer)
}
pub fn read_fd(fd: i32, buffer: &mut [u8]) -> core::result::Result<usize, isize> {
    let mut used = 0;
    while used < buffer.len() {
        let n = read(fd, &mut buffer[used..]);
        if n < 0 {
            return Err(n);
        }
        if n == 0 {
            return Ok(used);
        }
        used += n as usize;
    }
    Err(-75) // EOVERFLOW: never accept a truncated record.
}

pub struct Buffer<const N: usize> {
    pub bytes: [u8; N],
    pub len: usize,
}
impl<const N: usize> Buffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn clear(&mut self) {
        self.len = 0;
        self.bytes[0] = 0;
    }
    pub fn append(&mut self, text: &[u8]) -> Result<()> {
        if self.len + text.len() >= N {
            return Err("A fixed buffer is too small for this input.");
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text);
        self.len += text.len();
        self.bytes[self.len] = 0;
        Ok(())
    }
    pub fn number(&mut self, value: u64) -> Result<()> {
        write!(self, "{}", value).map_err(|_| "Numeric buffer overflow.")
    }
    pub fn data(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }
}
impl<const N: usize> Write for Buffer<N> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.append(text.as_bytes()).map_err(|_| fmt::Error)
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Statx {
    pub mask: u32,
    pub blksize: u32,
    pub attributes: u64,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
    pub spare: u16,
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub attrmask: u64,
    pub times: [u64; 8],
    pub rdevmajor: u32,
    pub rdevminor: u32,
    pub devmajor: u32,
    pub devminor: u32,
    pub rest: [u64; 14],
}
pub fn stat(directory: usize, path: *const u8, flags: usize) -> core::result::Result<Statx, isize> {
    let mut info: Statx = unsafe { core::mem::zeroed() };
    loop {
        let n = unsafe {
            call(
                332,
                directory,
                path as usize,
                flags,
                0x7ff,
                &mut info as *mut _ as usize,
                0,
            )
        };
        if n != -4 {
            return if n < 0 { Err(n) } else { Ok(info) };
        }
    }
}
pub fn same(a: &Statx, b: &Statx) -> bool {
    a.ino == b.ino && a.devmajor == b.devmajor && a.devminor == b.devminor
}

#[repr(C)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}
// Linux x86-64 UAPI layouts. Check all fields read by this program.
const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(size_of::<Fd>() == 4);
    assert!(align_of::<Fd>() == 4);
    assert!(size_of::<Tid>() == 4);
    assert!(size_of::<Statx>() == 256);
    assert!(align_of::<Statx>() == 8);
    assert!(offset_of!(Statx, uid) == 20);
    assert!(offset_of!(Statx, mode) == 28);
    assert!(offset_of!(Statx, ino) == 32);
    assert!(offset_of!(Statx, devmajor) == 136);
    assert!(offset_of!(Statx, devminor) == 140);
    assert!(size_of::<Timespec>() == 16);
    assert!(align_of::<Timespec>() == 8);
    assert!(offset_of!(Timespec, sec) == 0);
    assert!(offset_of!(Timespec, nsec) == 8);
};

pub fn now() -> u64 {
    let mut t = Timespec { sec: 0, nsec: 0 };
    if unsafe { call(228, 1, &mut t as *mut _ as usize, 0, 0, 0, 0) } != 0 {
        exit(1);
    }
    t.sec as u64 * 1_000_000_000 + t.nsec as u64
}
pub fn sleep_until(deadline: u64) {
    let t = Timespec {
        sec: (deadline / 1_000_000_000) as i64,
        nsec: (deadline % 1_000_000_000) as i64,
    };
    loop {
        let n = unsafe { call(230, 1, 1, &t as *const _ as usize, 0, 0, 0) };
        if n != -4 {
            if n < 0 {
                exit(1);
            }
            break;
        }
    }
}
pub fn decimal(text: &[u8]) -> Option<u64> {
    if text.is_empty() {
        return None;
    }
    let mut v = 0u64;
    for &b in text {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(v)
}
pub fn trim(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(text.len());
    let end = text
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    &text[start..end]
}
pub fn member(current: &[u8], root: &[u8]) -> bool {
    !root.is_empty()
        && (current == root
            || (current.starts_with(root) && current.get(root.len()) == Some(&b'/')))
}
pub unsafe fn cstr<'a>(ptr: *const u8) -> &'a [u8] {
    let mut n = 0;
    while *ptr.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(ptr, n)
}
pub unsafe fn environment<'a>(env: *const *const u8, key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while !(*env.add(i)).is_null() {
        let text = cstr(*env.add(i));
        if text.starts_with(key) && text.get(key.len()) == Some(&b'=') {
            return Some(&text[key.len() + 1..]);
        }
        i += 1;
    }
    None
}

// Compiler-generated memory operations use these bounded, allocation-free routines.
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dest: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    for i in 0..n {
        dest.cast::<u8>()
            .add(i)
            .write_volatile(src.cast::<u8>().add(i).read_volatile());
    }
    dest
}
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memset(
    dest: *mut core::ffi::c_void,
    value: i32,
    n: usize,
) -> *mut core::ffi::c_void {
    for i in 0..n {
        dest.cast::<u8>().add(i).write_volatile(value as u8);
    }
    dest
}
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memmove(
    dest: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    if (dest as usize) < (src as usize) {
        memcpy(dest, src, n);
    } else {
        for i in (0..n).rev() {
            dest.cast::<u8>()
                .add(i)
                .write_volatile(src.cast::<u8>().add(i).read_volatile());
        }
    }
    dest
}
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    for i in 0..n {
        let x = a.cast::<u8>().add(i).read_volatile();
        let y = b.cast::<u8>().add(i).read_volatile();
        if x != y {
            return x as i32 - y as i32;
        }
    }
    0
}
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn bcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    memcmp(a, b, n)
}
#[cfg(not(test))]
#[no_mangle]
pub unsafe extern "C" fn strlen(text: *const core::ffi::c_char) -> usize {
    let mut n = 0;
    while text.add(n).read_volatile() != 0 {
        n += 1;
    }
    n
}
#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn rust_eh_personality() {}
