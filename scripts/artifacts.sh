#!/usr/bin/env bash
# Shared build and release checks. The caller supplies die().

source_fingerprint() {
    local root=$1 file
    {
        printf 'steam-high-priority build format 1\n'
        for file in src/main.rs src/sys.rs src/dirent.rs src/game.rs src/poll.rs src/launch.rs scripts/build; do
            sha256sum "$root/$file" | cut -d ' ' -f1 || return 1
        done
    } | sha256sum | cut -d ' ' -f1
}

verify_binary() {
    local binary=$1 header segments dynamic symbols undefined
    [[ -f $binary && ! -L $binary && -s $binary ]] || die 'The runtime must be a nonempty regular file.'
    header=$(readelf -hW "$binary") || die 'Cannot read the runtime ELF header.'
    [[ $header == *'Advanced Micro Devices X86-64'* && $header == *ELF64* ]] || die 'Unexpected executable architecture.'
    segments=$(readelf -lW "$binary") || die 'Cannot read the runtime segments.'
    dynamic=$(readelf -dW "$binary") || die 'Cannot read the runtime dependencies.'
    if grep -q INTERP <<< "$segments" || grep -q NEEDED <<< "$dynamic"; then
        die 'The runtime must be static.'
    fi
    if grep -Eq 'LOAD .* RWE |GNU_STACK .* RWE ' <<< "$segments"; then
        die 'Writable executable mappings are not permitted.'
    fi
    undefined=$(nm -u "$binary") || die 'Cannot inspect unresolved runtime symbols.'
    [[ -z $undefined ]] || die 'The runtime contains unresolved symbols.'
    symbols=$(nm "$binary") || die 'Cannot inspect runtime symbols.'
    if grep -Eq '__rust_alloc|__rg_alloc|__rdl_alloc| malloc$| calloc$| realloc$' <<< "$symbols"; then
        die 'The runtime contains an allocator.'
    fi
}

# Parse release metadata as data. No release field is evaluated as shell code.
read_build_info() {
    local path=$1 line key value
    local -A info=()
    [[ -f $path && ! -L $path && $(stat -c %s "$path") -le 4096 ]] || die 'Missing or invalid release build-info.'
    while IFS= read -r line || [[ -n $line ]]; do
        [[ $line == *$'\t'* ]] || die 'Invalid release metadata record.'
        key=${line%%$'\t'*}
        value=${line#*$'\t'}
        case $key in
            format|version|commit|target|compiler|source|binary) ;;
            *) die 'Unknown release metadata field.' ;;
        esac
        [[ -n $value && $value != *$'\t'* && $value != *$'\r'* && ! -v info[$key] ]] || die 'Invalid or duplicate release metadata field.'
        info[$key]=$value
    done < "$path"
    [[ ${#info[@]} == 7 && ${info[format]:-} == 1 ]] || die 'Incomplete or unsupported release metadata.'
    [[ ${info[version]} =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || die 'Invalid release version.'
    [[ ${info[commit]} =~ ^[0-9a-f]{40}$ ]] || die 'Invalid release commit.'
    [[ ${info[target]} == x86_64-unknown-linux-gnu ]] || die 'Unsupported release target.'
    [[ ${info[compiler]} == 'rustc '* ]] || die 'Invalid release compiler record.'
    [[ ${info[source]} =~ ^[0-9a-f]{64}$ && ${info[binary]} =~ ^[0-9a-f]{64}$ ]] || die 'Invalid release checksum.'
    RELEASE_SOURCE=${info[source]}
    RELEASE_BINARY=${info[binary]}
    RELEASE_COMPILER=${info[compiler]}
}
