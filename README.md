# steam-high-priority

Run Steam and everything it launches at nice **−19**. A small Rust poller checks the Steam cgroup every **two seconds** and corrects thread priorities changed by Wine or a game.

The policy covers Steam, web helpers, downloads, Proton, Wine, games, and their worker threads. It uses one static executable with fixed-size memory and native Linux syscalls.

## Measured overhead

Across four 16-second runs with a 129-thread test workload on an Intel Core i7-7700K, the poller used **204 KiB of resident memory** and averaged **0.0045% of one logical CPU**.

## Requirements

- Linux x86-64 with cgroup v2.
- systemd 250 or newer with `systemd-run --expand-environment=no`.
- A working systemd user manager and user bus.
- Native Steam at `/usr/bin/steam` or `/usr/games/steam`.
- Rust with the x86-64 GNU target and `rust-lld`, as supplied by a normal rustup installation.
- Bash, GNU coreutils, binutils (`readelf`, `nm`), and util-linux (`flock`, `prlimit`, `runuser`).
- `/usr/local/bin` before the distribution's Steam directory in `PATH`.

## Install

From the cloned repository:

```sh
cd steam-high-priority
./install
```

The script takes no arguments and requests sudo when needed. `sudo ./install` also works. Compilation uses the invoking user's Rust installation; a direct root invocation uses root's installation.

The installer builds locally and installs root-owned files. It grants systemd user managers permission to request nice −19 and preserves stronger existing allowances. Steam runs as the logged-in user at −19; the poller runs at nice 0.

Close any existing Steam session, open a new terminal, and launch:

```sh
steam
```

Use the same command in PATH-based window-manager bindings. Steam URLs work too:

```sh
steam steam://open/games
```

Commands and shortcuts that call `/usr/bin/steam` directly bypass the managed launcher. Desktop entries and MIME associations stay unchanged. If the installer reports an existing user-local Steam wrapper, move or remove that wrapper before installation.

### Reinstall and upgrade

```sh
./install
```

An unchanged installation keeps the same contents, ownership, permissions, link targets, and modification times. The installer reuses a verified binary with the same source fingerprint.

Reinstall repairs missing or damaged project-owned files. Changed source produces a new executable, installed through atomic replacement. Running Steam sessions continue with their existing version; the next session uses the update.

Conflicting files are preserved and reported. The installer stages changes and restores the preceding files and recorded runtime limits after a command failure or catchable interruption.

## Uninstall

Close Steam, then run:

```sh
./uninstall
```

The script takes no arguments and requests sudo when needed. It removes project-owned files and the priority-permission drop-in. Steam files, games, saves, shader caches, and launch options stay unchanged. Running uninstall again leaves the system unchanged.

Recorded user-manager limits are restored when the manager identity and applied limits still match. Later administrator changes are preserved. Managers started after installation receive their normal allowance at their next login. Existing processes retain their inherited limits until they exit.

Uninstall asks you to close any remaining managed Steam sessions before removal. Modified installation files can be repaired with `./install` first.

Open a new terminal afterward to refresh the shell's command lookup.

## How it works

```text
systemd --user
└── app.slice
    ├── steam-high-priority-session.service  nice −19
    │   ├── Steam and web helpers
    │   ├── Proton / Wine
    │   └── Games and worker threads
    └── steam-high-priority.service          nice 0
        └── thread priority poller
```

The launcher creates a transient session service with `ExitType=cgroup`. Child processes and threads inherit group membership. The session remains active until its last process exits. The poller's `BindsTo` and `After` dependencies tie its lifetime to that session.

Every two seconds, the poller reads `cgroup.threads` in the Steam group and its nested groups. It checks user ownership and group membership, then changes priorities that differ from −19. It sleeps on an absolute monotonic deadline between passes. Short-lived threads can finish between polls; later priority changes are corrected on the next pass.

The root directory and `cgroup.events` descriptors are reused between passes. A descriptor failure clears the pair and permits one reopen attempt in that pass. Further attempts wait for the next deadline. Failed thread updates leave the remaining checks running. Persistent errors are logged once, followed by a recovery message after a successful pass.

The launcher serializes simultaneous requests and forwards subsequent arguments through Steam's command interface. Arguments remain separate, and the selected display, locale, PATH, and XDG environment values are preserved.

The executable is approximately 42 KiB. Its storage limits are 32 nested cgroup levels, 4096-byte path buffers, and 255 launch arguments including systemd options. Buffer overflow returns an error. The launcher uses the installed systemd and Steam forwarding commands.

CPU affinity, scheduling classes, and cgroup CPU weights stay unchanged.

## Installed files

| Path | Purpose |
|---|---|
| `/usr/local/bin/steam` | Terminal/window-manager entry point |
| `/usr/local/libexec/steam-high-priority/steam-high-priority` | Static Rust executable |
| `/usr/local/libexec/steam-high-priority/steam-session` | Launcher link |
| `/usr/local/libexec/steam-high-priority/steam-priority` | Poller link |
| `/usr/local/libexec/steam-high-priority/steam-path` | Distribution Steam launcher path |
| `/usr/local/libexec/steam-high-priority/install.manifest` | Source fingerprint, build record, file hashes, permissions, and links |
| `/usr/local/libexec/steam-high-priority/manager-limits.tsv` | Original runtime limits and manager identities |
| `/etc/systemd/user/steam-high-priority.service` | User polling service |
| `/etc/systemd/system/user@.service.d/50-steam-high-priority.conf` | User-manager nice allowance |

Installation and removal share a root-owned lock under `/run/steam-high-priority/`.

## Status

```sh
systemctl --user status steam-high-priority-session.service steam-high-priority.service
journalctl --user -u steam-high-priority.service
```

Steam starts when you launch it. After shutdown, the transient session unit is removed and the poller becomes inactive.

## License

GNU Affero General Public License version 3 (`AGPL-3.0-only`). See `LICENSE`.
