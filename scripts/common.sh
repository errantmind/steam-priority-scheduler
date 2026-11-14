#!/usr/bin/env bash
# Shared installation operations. Public entry points take no arguments.
set -Eeuo pipefail
umask 022
export LC_ALL=C
PREFIX=/usr/local/libexec/steam-high-priority
MANIFEST=$PREFIX/install.manifest
UNIT=/etc/systemd/user/steam-high-priority.service
DROPIN=/etc/systemd/system/user@.service.d/50-steam-high-priority.conf
LAUNCHER=/usr/local/bin/steam
FILES=("$PREFIX/steam-high-priority" "$PREFIX/steam-path" "$UNIT" "$DROPIN" "$PREFIX/manager-limits.tsv")
LINKS=("$PREFIX/steam-session" "$PREFIX/steam-priority" "$LAUNCHER")
TARGETS=("${FILES[@]}" "${LINKS[@]}" "$MANIFEST")
declare -A OLD_KIND=() OLD_MODE=() OLD_VALUE=()
OLD_SOURCE= OLD_COMPILER=
WORK= BUILD_DIR= TRANSACTION=0 RELOAD=0
CREATED_DIRS=()
MANAGERS=()

die() { printf 'steam-high-priority: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || die "Required command is missing: $1"; }
require_root() {
    (($# == 0)) || die 'This script takes no arguments.'
    if ((EUID != 0)); then
        need sudo
        exec sudo -- /bin/bash "$ENTRY"
    fi
    for cmd in flock mktemp stat sha256sum cmp install cp mv readlink systemctl prlimit getent runuser; do need "$cmd"; done
    safe_parents /run/steam-high-priority/lock
    if ! exists /run/steam-high-priority; then mkdir -m 700 /run/steam-high-priority; fi
    [[ -d /run/steam-high-priority && ! -L /run/steam-high-priority && $(stat -c %u:%a /run/steam-high-priority) == 0:700 ]] || die 'Unsafe installer lock directory.'
    [[ ! -L /run/steam-high-priority/lock ]] || die 'Unsafe installer lock.'
    exec 9<>/run/steam-high-priority/lock
    flock -x 9
}
exists() { [[ -e $1 || -L $1 ]]; }
hash() { sha256sum -- "$1" | cut -d ' ' -f 1; }
known_target() { local item; for item in "${FILES[@]}" "${LINKS[@]}"; do [[ $item != "$1" ]] || return 0; done; return 1; }
safe_parents() {
    local p=$1
    while [[ $p != / ]]; do
        p=$(dirname -- "$p")
        [[ ! -L $p ]] || die "A managed path has a symbolic-link parent: $p"
        if [[ -e $p ]]; then
            [[ -d $p && $(stat -c %u "$p") == 0 ]] || die "Unsafe parent directory: $p"
            local mode; mode=$(stat -c %a "$p")
            (( (8#$mode & 0022) == 0 )) || die "Parent directory is writable by other users: $p"
        fi
    done
}
load_manifest() {
    local path kind mode value extra seen=0
    for path in "${TARGETS[@]}"; do safe_parents "$path"; done
    if ! exists "$MANIFEST"; then
        for path in "${FILES[@]}" "${LINKS[@]}"; do
            ! exists "$path" || die "Existing file is not owned by this installation: $path"
        done
        return
    fi
    [[ -f $MANIFEST && ! -L $MANIFEST && $(stat -c %u "$MANIFEST") == 0 ]] || die 'Unsafe installation manifest.'
    [[ $(stat -c %a "$MANIFEST") == 644 ]] || die 'Unexpected installation manifest permissions.'
    while IFS=$'\t' read -r kind mode value path extra; do
        case $kind in
            format) [[ $mode == 1 && -z $value$path$extra && $seen == 0 ]] || die 'Unsupported manifest.'; seen=1 ;;
            source) [[ $mode =~ ^[0-9a-f]{64}$ && -z $OLD_SOURCE$value$path$extra ]] || die 'Invalid source fingerprint.'; OLD_SOURCE=$mode ;;
            compiler) [[ -n $mode && -z $OLD_COMPILER$value$path$extra ]] || die 'Invalid compiler record.'; OLD_COMPILER=$mode ;;
            file|link)
                known_target "$path" && [[ -z $extra && ! -v OLD_KIND["$path"] ]] || die 'Invalid manifest target.'
                if [[ $kind == file ]]; then
                    [[ $mode =~ ^(644|755)$ && $value =~ ^[0-9a-f]{64}$ ]] || die 'Invalid file record.'
                else
                    [[ $mode == - && ( $value == steam-high-priority || $value == "$PREFIX/steam-session" ) ]] || die 'Invalid link record.'
                fi
                OLD_KIND["$path"]=$kind; OLD_MODE["$path"]=$mode; OLD_VALUE["$path"]=$value ;;
            *) die 'Invalid installation manifest.' ;;
        esac
    done < "$MANIFEST"
    [[ $seen == 1 && -n $OLD_SOURCE && -n $OLD_COMPILER && ${#OLD_KIND[@]} == $((${#FILES[@]}+${#LINKS[@]})) ]] || die 'Incomplete installation manifest.'
    for path in "${FILES[@]}"; do [[ ${OLD_KIND[$path]:-} == file ]] || die 'Manifest file type mismatch.'; done
    for path in "${LINKS[@]}"; do [[ ${OLD_KIND[$path]:-} == link ]] || die 'Manifest link type mismatch.'; done
}
old_matches() {
    local p=$1
    if [[ ${OLD_KIND[$p]:-} == link ]]; then
        [[ -L $p && $(readlink -- "$p") == "${OLD_VALUE[$p]}" && $(stat -c %u "$p") == 0 ]]
    else
        [[ -f $p && ! -L $p && $(stat -c %u:%g:%a "$p") == "0:0:${OLD_MODE[$p]}" && $(hash "$p") == "${OLD_VALUE[$p]}" ]]
    fi
}
manager_list() {
    local unit rest output
    MANAGERS=()
    output=$(systemctl list-units --type=service --state=running --plain --no-legend 'user@*.service')
    while read -r unit rest; do
        if [[ $unit =~ ^user@([0-9]+)\.service$ ]]; then MANAGERS+=("${BASH_REMATCH[1]}"); fi
    done <<< "$output"
}
start_time() {
    local text
    [[ -r /proc/$1/stat ]] || return 1
    text=$(</proc/"$1"/stat); text=${text##*) }
    local fields; read -ra fields <<< "$text"
    printf '%s\n' "${fields[19]}"
}
limit_pair() { prlimit --pid "$1" --nice --noheadings --raw --output SOFT,HARD; }
valid_limit() { [[ $1 =~ ^[0-9]+$ || $1 == unlimited || $1 == infinity ]]; }
raise_limit() {
    valid_limit "$1" || die "Invalid nice allowance: $1"
    if [[ $1 == unlimited || $1 == infinity ]]; then printf 'unlimited'; elif (( $1 < 39 )); then printf 39; else printf '%s' "$1"; fi
}
record_limit_change() {
    local pid=$1 stamp=$2 oldsoft=$3 oldhard=$4 newsoft=$5 newhard=$6
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$stamp" "$oldsoft" "$oldhard" "$newsoft" "$newhard" >> "$WORK/runtime-undo"
    prlimit --pid "$pid" --nice="$newsoft:$newhard"
}
restore_runtime_undo() {
    local pid stamp soft hard appliedsoft appliedhard nowsoft nowhard
    [[ -f $WORK/runtime-undo ]] || return 0
    while IFS=$'\t' read -r pid stamp soft hard appliedsoft appliedhard; do
        [[ $(start_time "$pid" 2>/dev/null) == "$stamp" ]] || continue
        read -r nowsoft nowhard < <(limit_pair "$pid")
        [[ $nowsoft:$nowhard == "$appliedsoft:$appliedhard" ]] || continue
        prlimit --pid "$pid" --nice="$soft:$hard" || printf 'Could not restore manager %s limits.\n' "$pid" >&2
    done < "$WORK/runtime-undo"
}
reload_managers() {
    local uid name
    systemctl daemon-reload
    manager_list
    for uid in "${MANAGERS[@]}"; do
        [[ -S /run/user/$uid/bus ]] || continue
        name=$(getent passwd "$uid" | cut -d: -f1)
        [[ -n $name ]] || die "Cannot identify user manager $uid."
        runuser -u "$name" -- env XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
            /usr/bin/systemctl --user daemon-reload
    done
}
make_work() {
    WORK=$(mktemp -d /var/tmp/steam-high-priority.XXXXXXXX)
    chmod 700 "$WORK"
    mkdir "$WORK/desired" "$WORK/before"
}
begin_transaction() {
    local i p
    for i in "${!TARGETS[@]}"; do
        p=${TARGETS[$i]}
        if exists "$p"; then
            [[ ! -d $p ]] || die "A file target is a directory: $p"
            cp -a --no-dereference -- "$p" "$WORK/before/$i"
        fi
    done
    TRANSACTION=1
}
ensure_directory() {
    [[ -d $1 ]] && return
    ensure_directory "$(dirname -- "$1")"
    mkdir -m 755 -- "$1"; CREATED_DIRS+=("$1")
}
put_file() {
    local src=$1 p=$2 mode=$3 temp
    if [[ -f $p && ! -L $p && $(stat -c %u:%g:%a "$p") == "0:0:$mode" ]] && cmp -s -- "$src" "$p"; then return; fi
    ensure_directory "$(dirname -- "$p")"
    temp=$(mktemp "$(dirname -- "$p")/.shp.XXXXXXXX")
    install -o root -g root -m "$mode" -- "$src" "$temp"
    mv -Tf -- "$temp" "$p"
    [[ $p != "$UNIT" && $p != "$DROPIN" ]] || RELOAD=1
}
put_link() {
    local p=$1 target=$2 temp
    if [[ -L $p && $(readlink -- "$p") == "$target" && $(stat -c %u "$p") == 0 ]]; then return; fi
    ensure_directory "$(dirname -- "$p")"
    temp=$(mktemp "$(dirname -- "$p")/.shp.XXXXXXXX"); rm -- "$temp"
    ln -s -- "$target" "$temp"; mv -Tf -- "$temp" "$p"
}
cleanup() {
    local rc=$? i p temp
    trap - EXIT INT TERM HUP
    if ((TRANSACTION)); then
        printf 'Restoring the previous installation after an error.\n' >&2
        set +e
        for i in "${!TARGETS[@]}"; do
            p=${TARGETS[$i]}
            if exists "$WORK/before/$i"; then
                temp="$(dirname -- "$p")/.shp-restore-$$"
                cp -a --no-dereference -- "$WORK/before/$i" "$temp" && mv -Tf -- "$temp" "$p"
            else rm -f -- "$p"; fi
        done
        restore_runtime_undo
        for ((i=${#CREATED_DIRS[@]}-1;i>=0;i--)); do rmdir -- "${CREATED_DIRS[$i]}" 2>/dev/null; done
        if ((RELOAD)); then reload_managers; fi
    fi
    [[ -z $BUILD_DIR ]] || rm -rf -- "$BUILD_DIR"
    [[ -z $WORK ]] || rm -rf -- "$WORK"
    exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
