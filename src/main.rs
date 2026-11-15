#![no_std]
#![no_main]
#[cfg(not(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
)))]
compile_error!("steam-high-priority requires Linux x86-64.");
mod dirent;
mod launch;
mod poll;
mod sys;

core::arch::global_asm!(
    ".global _start",
    ".type _start,@function",
    "_start:",
    "xor rbp,rbp",
    "mov rdi,rsp",
    "and rsp,-16",
    "call entry",
    "ud2",
);

#[no_mangle]
unsafe extern "C" fn entry(stack: *const usize) -> ! {
    let argc = *stack;
    let argv = stack.add(1) as *const *const u8;
    let env = stack.add(argc + 2) as *const *const u8;
    let name = if argc > 0 {
        sys::cstr(*argv)
            .rsplit(|b| *b == b'/')
            .next()
            .unwrap_or(b"")
    } else {
        b""
    };
    let result = if name == b"steam-priority" {
        if argc != 1 {
            Err("The priority service takes no arguments.")
        } else {
            poll::run()
        }
    } else {
        launch::run(argc, argv, env)
    };
    match result {
        Ok(code) => sys::exit(code),
        Err(message) => {
            sys::error(message);
            sys::exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys::error("Internal bounds check failed. No further policy changes will be made.");
    sys::exit(1)
}
