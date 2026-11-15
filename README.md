# steam-high-priority

Run Steam games and their supporting processes at nice **−19**. A small Rust poller checks Steam application scopes every **two seconds** and corrects thread priorities changed by Wine or a game.

Steam starts at normal priority. Its client and web helpers retain their own priority settings. Games and Proton/Wine processes are covered in separate application scopes or, through application-marker checks, inside the Steam client group. The policy uses one static executable with fixed-size memory and native Linux syscalls.

## Performance design

- **Static `no_std` Rust executable** with direct Linux syscalls and no libc or shared-library dependencies.
- **Zero heap allocations**, using fixed-size buffers initialized before polling.
- **Targeted cgroup scans** discover Steam application scopes and skip unrelated groups.
- **One priority query per game thread**, with ownership and membership checks only when correction is needed. Fallback game identification runs once per process per pass.
- **Cached file descriptors** reduce repeated opens, with bounded recovery after failures.
- **One poller per user**, sleeping between two-second checks and remaining active until Steam and its application scopes are empty.

## Measured overhead

In an 8-second test with 129 game threads in one application scope and 20 unrelated sibling groups on an Intel Core i7-7700K, the poller ran at nice +10, used **212 KiB of resident memory**, and averaged **0.0104% of one logical CPU**.

## Requirements

- Linux x86-64 with cgroup v2.
- systemd 250 or newer with `systemd-run --expand-environment=no`.
- A working systemd user manager and user bus.
- Native Steam at `/usr/bin/steam` or `/usr/games/steam`.
- Bash, GNU coreutils, binutils (`readelf`, `nm`), and util-linux (`flock`, `prlimit`, `runuser`).
- `/usr/local/bin` before the distribution's Steam directory in `PATH`.

## Install

Download the Linux x86-64 archive and `SHA256SUMS` from [GitHub Releases](https://github.com/errantmind/steam-high-priority/releases). Save both files in the same directory. For `v0.1.0`:

```sh
sha256sum --check SHA256SUMS
tar -xzf steam-high-priority-v0.1.0-linux-x86_64.tar.gz
cd steam-high-priority
./install
```

The release includes a prebuilt binary, its exact runtime source, build scripts, and license. `build-info` records the version, commit, compiler, target, and source/binary checksums. Installation verifies the bundled binary and source. Rust is required only for source builds.

The script takes no arguments and requests sudo when needed. `sudo ./install` also works. Keep the extracted directory for `./uninstall`.

The installer installs root-owned files. It grants systemd user managers permission to request nice −19 and preserves stronger existing allowances. Steam runs as the logged-in user. Steam starts at nice 0 and the poller at nice +10. The poller applies −19 to eligible game threads.

Close Steam and its games, open a new terminal, and launch:

```sh
steam
```

Use the same command in PATH-based window-manager bindings. Steam URLs work too:

```sh
steam steam://open/games
```

Commands and shortcuts that call `/usr/bin/steam` directly bypass the managed launcher. Desktop entries and MIME associations stay unchanged. If the installer reports an existing user-local Steam wrapper, move or remove that wrapper before installation.

### Install from source

Source builds require Rust with the x86-64 GNU target and `rust-lld`, as supplied by a normal rustup installation.

```sh
git clone https://github.com/errantmind/steam-high-priority.git
cd steam-high-priority
./install
```

Compilation uses the invoking user's Rust installation; a direct root invocation uses root's installation.

### Reinstall and upgrade

```sh
./install
```

Extract the new release into a clean directory, then run its `./install`. An unchanged installation keeps the same contents, ownership, permissions, link targets, and modification times.

Release installation uses the exact bundled binary. Source installation reuses a verified binary with the same source fingerprint, or compiles changed source. Reinstall repairs missing or damaged project-owned files. Executables are replaced atomically. Close Steam and its games before activating an update; a fresh session establishes the new priority policy.

Conflicting files are preserved and reported. The installer stages changes and restores the preceding files and recorded runtime limits after a command failure or catchable interruption.

## Uninstall

Close Steam and its games, then run:

```sh
./uninstall
```

The script takes no arguments and requests sudo when needed. It removes project-owned files and the priority-permission drop-in. Steam files, games, saves, shader caches, and launch options stay unchanged. Running uninstall again leaves the system unchanged.

Recorded user-manager limits are restored when the manager identity and applied limits still match. Later administrator changes are preserved. Managers started after installation receive their normal allowance at their next login. Existing processes retain their inherited limits until they exit.

Uninstall checks the managed client groups and Steam application scopes, then stops any remaining poller before removal. Modified installation files can be repaired with `./install` first.

Open a new terminal afterward to refresh the shell's command lookup.

## How it works

```text
systemd --user
└── app.slice
    ├── steam-high-priority-session.service  starts at nice 0
    │   ├── Steam and web helpers            retain their priorities
    │   └── Fallback games / Proton / Wine   corrected to −19
    ├── app-steam-app<appid>-<pid>.scope      game threads corrected to −19
    │   └── Game / Proton / Wine
    └── steam-high-priority.service          nice +10
        └── thread priority poller
```

The launcher creates a transient client service with `ExitType=cgroup`, then starts the polling service. A new client session restarts the poller once; forwarding requests to an existing session leaves it running. The poller determines its lifetime from populated client and application groups, so it continues when the client exits before a game.

Every two seconds, the poller discovers sibling scopes named `app-steam-app<appid>-<pid>.scope`, with positive numeric identifiers. It reads their `cgroup.threads` files and nested groups. A priority already at −19 takes one query. A correction also checks user ownership, group membership, and task identity.

For games left in the client group, the poller reads `cgroup.procs` and identifies each process once per pass. Executables named `steam` and `steamwebhelper` are excluded; Wine's `steam.exe` is distinct. A positive numeric `SteamAppId`, `SteamGameId`, or `STEAM_COMPAT_APP_ID` environment value identifies an eligible fallback process. Processes without these markers retain their priorities. Marker reads stream through a fixed buffer; environment contents are not logged.

Discovery and game traversal reuse the same buffers. The application-slice descriptor and the client's directory/state pair are cached. A failed discovery pass or client cache operation permits one retry; further attempts wait for the next deadline. An application-scope failure leaves other scopes eligible for scanning. Failed thread updates also leave the remaining checks running. Persistent errors are logged once, followed by a recovery message after a successful pass. Failed population checks keep the poller active for a later retry.

The poller sleeps on an absolute monotonic deadline. Short-lived threads can finish between polls; new threads and later priority changes are corrected on the next pass.

The launcher serializes simultaneous requests and forwards subsequent arguments through Steam's command interface. Arguments remain separate, and the selected display, locale, PATH, and XDG environment values are preserved.

The executable is approximately 51 KiB. It uses `no_std`, fixed buffers, and direct Linux syscalls. The poller, fallback classifier, and directory parser forbid unsafe code. The poller's native calls use syscall wrappers. Descriptor ownership is explicit, and Linux structure layouts are checked at compile time.

Storage limits are 32 nested cgroup levels, 4096-byte path buffers, and 255 launch arguments including systemd options. Buffer overflow returns an error. The launcher uses the installed systemd and Steam forwarding commands. If captured helper output exceeds its buffer or cannot be read, the launcher terminates and reaps that helper.

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

Steam starts when you launch it. After the client and all recognized game groups are empty, the poller becomes inactive.

## Releases

Use annotated version tags on reviewed `master` commits. Start with `v0.1.0`; use patch versions for fixes and minor versions for new functionality or changed behavior.

```sh
git tag -a v0.1.0 -m "steam-high-priority v0.1.0"
git push origin v0.1.0
```

The release workflow builds with Rust **1.100.0**, verifies the static executable, and creates a draft release containing:

- `steam-high-priority-v0.1.0-linux-x86_64.tar.gz`
- `SHA256SUMS`

Download the draft archive and check installation, identical reinstall, Steam operation, and repeated uninstall before publishing. Add a short change summary to the draft. Published tags and assets remain unchanged; corrections receive a new version.

For a local package build with that compiler installed:

```sh
RUSTUP_TOOLCHAIN=1.100.0 ./scripts/package v0.1.0
```

Files are written under `build/release/`. Packaging includes the application, corresponding source/build recipe, installer, uninstaller, systemd units, README, and license.

## License

GNU Affero General Public License version 3 (`AGPL-3.0-only`). See `LICENSE`.
