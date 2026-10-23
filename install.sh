#!/bin/sh
# Auditable installation and guided connection; access grants require explicit approval.
# Parse the complete function before running it: a fragmented pipe remains code,
# never confirmation or wizard input. Interactive input uses the controlling TTY.
main() {
set -eu
umask 077
DEFAULT_VERSION=v0.4.1
RELEASE_ROOT=https://github.com/antoine-berger/tv-control/releases/download
guided=no
if [ "$#" -eq 0 ]; then guided=yes; fi
version=$DEFAULT_VERSION
version_supplied=no
prefix=
archive_input=
checksum_input=
expected_pin=
mode=install
update=no
noninteractive=no
interactive=no
setup_requested=no
if [ "$guided" = yes ]; then interactive=yes; setup_requested=yes; fi
json=no
dry=no
work=
activation=
lock=
lock_owned=no
result_emitted=no
uid=
usage() {
    cat <<'EOF'
Usage: sh install.sh [options]
  --version vX.Y.Z       Pin a release (default v0.4.1; never resolves latest)
  --prefix ABSOLUTE_DIR  Dedicated install root (default ~/.local/share/tv-control)
  --non-interactive     Install without prompts; never runs setup or device access
  --interactive         Ask before installation; optionally offer guided setup
  --setup               Explicitly start the guided wizard after interactive installation
  --json                Non-interactive structured result on stdout
  --dry-run             Read-only plan, without downloads or filesystem changes
  --update              Add a version to an existing owned installation, activate atomically
  --uninstall           Verify and remove only recorded unmodified installation files
  --archive FILE        Use an explicit local archive (offline/air-gapped mode)
  --checksum FILE       Matching SHA-256 file (default FILE.sha256)
  --sha256 HEX          Optional independently supplied expected archive digest
  --help                Show this text
No sudo or shell startup changes. Guided connection asks before granting access.
EOF
}
escape_json() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
result() {
    result_emitted=yes
    if [ "$json" = yes ]; then
        printf '{"status":"%s","version":"%s","prefix":"%s","setup_started":false,"path_modified":false}\n' "$1" "$(escape_json "$version")" "$(escape_json "$prefix")"
    else
        printf '%s\n' "$2"
        if [ "$1" != cancelled ]; then printf 'Installation directory: %s\n' "$prefix"; fi
    fi
}
fail() {
    result_emitted=yes
    if [ "$json" = yes ]; then printf '{"status":"error","code":"%s"}\n' "$1"; else printf 'tv-control installer: %s\n' "$1" >&2; fi
    if [ "$json" != yes ] && [ "$1" = interactive_terminal_required ]; then
        printf 'Run this interactive command in a terminal, or use --json/--non-interactive without --setup. No installation changes were made.\n' >&2
    fi
    exit 1
}
cleanup() {
    code=$?
    trap - 0 HUP INT TERM
    if [ "$code" -ne 0 ] && [ "$result_emitted" != yes ]; then
        if [ "$json" = yes ]; then printf '{"status":"error","code":"installer_operation_failed"}\n'; else printf 'tv-control installer: installer_operation_failed\n' >&2; fi
    fi
    if [ -n "$activation" ] && [ -f "$activation" ] && [ ! -L "$activation" ]; then rm "$activation"; fi
    if [ -n "$work" ] && [ -d "$work" ] && [ ! -L "$work" ]; then
        case "$work" in "$parent"/.tv-control-stage.*) rm -rf "$work" ;; esac
    fi
    if [ "$lock_owned" = yes ]; then rmdir "$lock" 2>/dev/null || :; fi
    exit "$code"
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 129' HUP
trap 'exit 143' TERM
need_value() { [ "$#" -ge 2 ] || fail option_value_missing; }
for argument do
    if [ "$argument" = --json ]; then json=yes; noninteractive=yes; fi
done
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) need_value "$@"; version=$2; version_supplied=yes; shift ;;
        --prefix) need_value "$@"; prefix=$2; shift ;;
        --archive) need_value "$@"; archive_input=$2; shift ;;
        --checksum) need_value "$@"; checksum_input=$2; shift ;;
        --sha256) need_value "$@"; expected_pin=$2; shift ;;
        --non-interactive) noninteractive=yes ;;
        --interactive) interactive=yes ;;
        --setup) setup_requested=yes; interactive=yes ;;
        --json) json=yes; noninteractive=yes ;;
        --dry-run) dry=yes ;;
        --update) update=yes ;;
        --uninstall) mode=uninstall ;;
        --help|-h) usage; exit 0 ;;
        *) fail unknown_option ;;
    esac
    shift
done
[ "$interactive" != yes ] || [ "$noninteractive" != yes ] || fail conflicting_interaction_modes
if [ "$interactive" != yes ] && [ "$noninteractive" != yes ]; then
    if [ -t 0 ] && [ -t 2 ]; then interactive=yes; else fail non_interactive_flag_required; fi
fi
if [ -z "$prefix" ]; then
    [ -n "${HOME:-}" ] || fail home_unavailable
    prefix=$HOME/.local/share/tv-control
fi
case "$prefix" in /*) ;; *) fail prefix_must_be_absolute ;; esac
case "$prefix" in /|*/../*|*/..|*/./*|*/.|*//*) fail unsafe_prefix ;; esac
prefix=${prefix%/}
[ "$(printf '%s' "$prefix" | LC_ALL=C tr -d '\000-\037\177')" = "$prefix" ] || fail control_character_in_prefix
valid_version() { [ "$(printf '%s' "$1" | LC_ALL=C tr -d '\000-\037\177')" = "$1" ] && printf '%s\n' "$1" | LC_ALL=C grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$'; }
valid_version "$version" || fail invalid_version
[ "$update" != yes ] || [ "$version_supplied" = yes ] || fail update_requires_explicit_version
valid_hash() { [ "${#1}" -eq 64 ] || return 1; case "$1" in *[!0-9a-fA-F]*) return 1 ;; esac; }

[ -z "$expected_pin" ] || valid_hash "$expected_pin" || fail invalid_sha256_pin
# Every existing component must be a directory rather than a symlink.
check_ancestors() {
    checked=$1
    while [ "$checked" != / ]; do
        [ ! -L "$checked" ] || fail symlink_path_refused
        if [ -e "$checked" ]; then [ -d "$checked" ] || fail path_component_not_directory; fi
        checked=${checked%/*}; [ -n "$checked" ] || checked=/
    done
}
uid=$(id -u)
check_ancestors "$prefix"
parent=${prefix%/*}; [ -n "$parent" ] || parent=/
# The immediate publication parent must belong to this user and not be writable by others.
check_private_parent() {
    [ -d "$parent" ] || fail parent_missing
    [ -n "$(find "$parent" -prune -user "$uid" -print)" ] || fail parent_not_owned_by_user
    [ -z "$(find "$parent" -prune \( -perm -0020 -o -perm -0002 \) -print)" ] || fail parent_writable_by_others
}
hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum < "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 < "$1" | awk '{print $1}'
    elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 < "$1" | awk '{print $NF}'
    else fail sha256_tool_missing; fi
}
allowed_member() {
    case "$1" in
        tv-control|LICENSE|README.md|PLAN.md|install.sh|examples/config.json|examples/mcp.json|examples/discovery.json|benchmarks/README.md|benchmarks/host-macos-arm64.json|docs/ACCEPTANCE.md|docs/ADAPTERS.md|docs/ADR.md|docs/CLI.md|docs/CONFIGURATION.md|docs/DEVELOPMENT.md|docs/INSTALLATION.md|docs/MCP.md|docs/ONBOARDING.md|docs/REVIEW.md|docs/ROUTER.md) return 0 ;;
        *) return 1 ;;
    esac
}
verify_installation() {
    if ! { [ -d "$prefix" ] && [ ! -L "$prefix" ]; }; then fail installation_missing; fi
    if ! { [ -f "$prefix/.tv-control-owner" ] && [ ! -L "$prefix/.tv-control-owner" ]; }; then fail prefix_not_owned_installation; fi
    [ "$(sed -n '1p' "$prefix/.tv-control-owner")" = TV_CONTROL_INSTALL_V1 ] || fail prefix_not_owned_installation
    [ -n "$(find "$prefix" -prune -user "$uid" -print)" ] || fail installation_owner_mismatch
    [ -z "$(find "$prefix" -type l -print)" ] || fail symlink_in_installation
    [ -z "$(find "$prefix" ! -type f ! -type d -print)" ] || fail unexpected_installation_entry
    recorded_wrapper=$(sed -n '2p' "$prefix/.tv-control-owner")
    valid_hash "$recorded_wrapper" || fail invalid_installation_marker
    [ "$(hash_file "$prefix/tv-control")" = "$recorded_wrapper" ] || fail launcher_modified
    active=$(cat "$prefix/current"); valid_version "$active" || fail invalid_active_version
    [ -d "$prefix/versions/$active" ] || fail active_version_missing
    : > "$work/expected-files"; : > "$work/expected-dirs"
    printf '%s\n' "$prefix/.tv-control-owner" "$prefix/current" "$prefix/tv-control" > "$work/expected-files"
    printf '%s\n' "$prefix" "$prefix/versions" > "$work/expected-dirs"
    for version_dir in "$prefix"/versions/*; do
        if ! { [ -d "$version_dir" ] && [ ! -L "$version_dir" ]; }; then fail unexpected_version_entry; fi
        stored_version=${version_dir##*/}; valid_version "$stored_version" || fail invalid_installed_version
        [ "$(sed -n '1p' "$version_dir/.receipt")" = "$stored_version" ] || fail receipt_mismatch
        valid_hash "$(sed -n '3p' "$version_dir/.receipt")" || fail invalid_receipt_digest
        printf '%s\n' "$version_dir/.receipt" "$version_dir/.manifest" >> "$work/expected-files"
        printf '%s\n' "$version_dir" "$version_dir/docs" "$version_dir/examples" "$version_dir/benchmarks" >> "$work/expected-dirs"
        [ -s "$version_dir/.manifest" ] || fail manifest_missing
        while IFS=' ' read -r digest member extra; do
            if ! { valid_hash "$digest" && [ -z "$extra" ] && allowed_member "$member"; }; then fail invalid_manifest; fi
            if ! { [ -f "$version_dir/$member" ] && [ ! -L "$version_dir/$member" ]; }; then fail installed_file_missing; fi
            [ "$(hash_file "$version_dir/$member")" = "$digest" ] || fail installed_file_modified
            printf '%s\n' "$version_dir/$member" >> "$work/expected-files"
        done < "$version_dir/.manifest"
    done
    LC_ALL=C sort "$work/expected-files" > "$work/expected-files.sorted"
    find "$prefix" -type f -print | LC_ALL=C sort > "$work/actual-files"
    cmp -s "$work/expected-files.sorted" "$work/actual-files" || fail unexpected_installation_files
    LC_ALL=C sort "$work/expected-dirs" > "$work/expected-dirs.sorted"
    find "$prefix" -type d -print | LC_ALL=C sort > "$work/actual-dirs"
    cmp -s "$work/expected-dirs.sorted" "$work/actual-dirs" || fail unexpected_installation_directories
}
run_setup() {
        config_parent=${XDG_CONFIG_HOME:-$HOME/.config}/tv-control
        case "$config_parent" in /*) ;; *) fail config_directory_must_be_absolute ;; esac
        check_ancestors "$config_parent"; mkdir -p "$config_parent"
        # Installation is complete before the long-running wizard. A crash in setup
        # must not leave an installer lock or staging directory behind.
        if [ -n "$work" ]; then
            case "$work" in "$parent"/.tv-control-stage.*) rm -rf "$work" ;; *) fail unsafe_stage ;; esac
            work=
        fi
        if [ "$lock_owned" = yes ]; then rmdir "$lock"; lock_owned=no; fi
        saved_configuration=no
        if [ -e "$config_parent/tv-control.local.json" ] || [ -L "$config_parent/tv-control.local.json" ]; then
            saved_configuration=yes
            printf 'Updating the existing controller; keeping your saved TV and connection settings.\n' >&3
        fi
        # The Rust setup reader validates the saved file and dependencies before
        # returning without discovery. Do not trust shell existence as validation.
        set -- setup --resume --output "$config_parent/tv-control.local.json"
        case "$version" in v0.2.0|v0.2.1|v0.2.2) ;; *) set -- "$@" --guided ;; esac
        "$prefix/versions/$version/tv-control" "$@" <&3 >&3 2>&3 3<&-
        case "$version" in v0.2.*) ;; *) "$prefix/versions/$version/tv-control" onboard "$config_parent/tv-control.local.json" --launcher "$prefix/tv-control" <&3 >&3 2>&3 3<&- ;; esac
        if [ "$saved_configuration" = yes ]; then
            printf 'Search for other devices without changing your saved TV? [y/N]: ' >&3
            if IFS= read -r answer <&3 && [ "$answer" = y ]; then
                # Discovery is list-only and selects a LAN explicitly. It cannot
                # replace configuration, pair, or authorize another device.
                if ! "$prefix/versions/$version/tv-control" discover <&3 >&3 2>&3 3<&-; then
                    printf 'Optional discovery did not complete. Your saved controller settings were not changed.\n' >&3
                fi
            fi
        fi
}
if [ "$mode" != uninstall ]; then
    os=$(uname -s); machine=$(uname -m)
    case "$machine" in arm64|aarch64) architecture=aarch64 ;; x86_64|amd64) architecture=x86_64 ;; *) fail unsupported_architecture ;; esac
    case "$os:$architecture" in
        Darwin:aarch64) target=aarch64-apple-darwin ;;
        Darwin:x86_64) fail macos_intel_unsupported ;;
        Linux:*) target=$architecture-linux-musl ;;
        *) fail unsupported_os ;;
    esac
fi
if [ "$dry" = yes ]; then result planned 'Dry run: no download, installation, setup or authentication performed.'; exit 0; fi
if [ "$interactive" = yes ]; then
    # Test the redirect in a subshell: failed exec redirects can terminate dash.
    if ! ( : <>/dev/tty ) 2>/dev/null; then fail interactive_terminal_required; fi
    exec 3<>/dev/tty
    [ -t 3 ] || fail interactive_terminal_required
    printf 'Operation: %s; release: %s; directory: %s\nContinue? Type yes: ' "$mode" "$version" "$prefix" >&3
    IFS= read -r answer <&3 || { result cancelled 'Cancelled; no changes made.'; exit 2; }
    [ "$answer" = yes ] || { result cancelled 'Cancelled; no changes made.'; exit 2; }
fi
# Parent creation is user-scoped; no privileged commands are used.
mkdir -p "$parent" || fail parent_creation_failed
check_ancestors "$parent"; check_private_parent
lock=$parent/.${prefix##*/}.install-lock
mkdir "$lock" 2>/dev/null || fail installation_locked
lock_owned=yes
work=$(mktemp -d "$parent/.tv-control-stage.XXXXXXXX") || fail temporary_directory_failed
if [ "$mode" = uninstall ]; then
    verify_installation
    version=$active
    config_parent=${XDG_CONFIG_HOME:-$HOME/.config}/tv-control
    case "$version" in v0.2.*) ;; *)
        if [ -f "$config_parent/connection/tunnel/.tv-control-owner" ]; then
            "$prefix/versions/$version/tv-control" disconnect "$config_parent/tv-control.local.json" --json --launcher "$prefix/tv-control" > /dev/null || fail controller_stop_failed_installation_preserved
        fi
        ;;
    esac
    # Only remove exact verified files; never recursively remove an installation root.
    while IFS= read -r owned_file; do rm "$owned_file" || fail uninstall_file_failed; done < "$work/expected-files"
    for version_dir in "$prefix"/versions/*; do
        rmdir "$version_dir/docs" "$version_dir/examples" "$version_dir/benchmarks" "$version_dir" || fail uninstall_directory_not_empty
    done
    rmdir "$prefix/versions" "$prefix" || fail uninstall_directory_not_empty
    result uninstalled 'Removed verified installation files. User configuration was not touched.'
    exit 0
fi
if [ -e "$prefix" ]; then
    verify_installation
    if [ "$setup_requested" = yes ] && [ "$active" = "$version" ]; then
        if [ -n "$archive_input" ] || [ -n "$checksum_input" ]; then fail setup_reuse_refuses_archive_override; fi
        if [ -n "$expected_pin" ]; then
            stored_digest=$(sed -n '3p' "$prefix/versions/$version/.receipt")
            [ "$(printf '%s' "$stored_digest" | tr 'A-F' 'a-f')" = "$(printf '%s' "$expected_pin" | tr 'A-F' 'a-f')" ] || fail checksum_mismatch
        fi
        result already_installed 'Existing installation verified; checking saved configuration and controller.'
        run_setup
        exit 0
    fi
    if [ "$guided" = yes ]; then update=yes; fi
    [ "$update" = yes ] || fail installation_exists_use_update
else
    [ "$update" = no ] || fail installation_missing
fi
asset=tv-control-$target.tar.gz
if [ -n "$archive_input" ]; then
    if ! { [ -f "$archive_input" ] && [ ! -L "$archive_input" ]; }; then fail local_archive_missing_or_symlink; fi
    [ -n "$checksum_input" ] || checksum_input=$archive_input.sha256
    if ! { [ -f "$checksum_input" ] && [ ! -L "$checksum_input" ]; }; then fail checksum_missing_or_symlink; fi
    cp "$archive_input" "$work/archive.tar.gz"; cp "$checksum_input" "$work/archive.sha256"
else
    [ -z "$checksum_input" ] || fail checksum_requires_local_archive
    command -v curl >/dev/null 2>&1 || fail curl_missing
    for download in "$asset" "$asset.sha256"; do
        case "$download" in *.sha256) output=$work/archive.sha256 ;; *) output=$work/archive.tar.gz ;; esac
        curl -q --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 10 --max-time 60 --max-filesize 16777216 --output "$output" "$RELEASE_ROOT/$version/$download" || fail https_download_failed
    done
fi
[ "$(awk 'END {print NR}' "$work/archive.sha256")" = 1 ] || fail invalid_checksum_file
IFS=' ' read -r expected checksum_name extra < "$work/archive.sha256" || fail invalid_checksum_file
if ! { valid_hash "$expected" && [ "$checksum_name" = "$asset" ] && [ -z "$extra" ]; }; then fail invalid_checksum_file; fi
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
[ -z "$expected_pin" ] || [ "$expected" = "$(printf '%s' "$expected_pin" | tr 'A-F' 'a-f')" ] || fail pinned_checksum_mismatch
[ "$(hash_file "$work/archive.tar.gz")" = "$expected" ] || fail archive_checksum_mismatch
# Never extract a tar archive into a directory. Inspect it, then stream each allowlisted
# regular member to a file we create inside a private stage; archive links/paths are ignored.
[ "$(wc -c < "$work/archive.tar.gz" | tr -d ' ')" -le 16777216 ] || fail compressed_archive_too_large
(ulimit -f 32768; gzip -dc "$work/archive.tar.gz" > "$work/archive.tar") 2>/dev/null || fail archive_decompression_failed
LC_ALL=C tar -tf "$work/archive.tar" > "$work/members" 2>/dev/null || fail invalid_archive
LC_ALL=C tar -tvf "$work/archive.tar" > "$work/types" 2>/dev/null || fail invalid_archive
awk 'substr($0,1,1)!="-" {exit 1}' "$work/types" || fail archive_links_or_special_files_refused
count=$(wc -l < "$work/members" | tr -d ' ')
{ [ "$count" -eq 19 ] || [ "$count" -eq 21 ]; } || fail archive_member_count_invalid
[ "$(LC_ALL=C sort -u "$work/members" | wc -l | tr -d ' ')" = "$count" ] || fail duplicate_archive_member
while IFS= read -r member; do allowed_member "$member" || fail unsafe_archive_member; done < "$work/members"
for required in tv-control LICENSE README.md PLAN.md examples/config.json examples/mcp.json examples/discovery.json benchmarks/README.md benchmarks/host-macos-arm64.json docs/ACCEPTANCE.md docs/ADAPTERS.md docs/ADR.md docs/CLI.md docs/CONFIGURATION.md docs/DEVELOPMENT.md docs/MCP.md docs/ONBOARDING.md docs/REVIEW.md docs/ROUTER.md; do
    grep -Fx "$required" "$work/members" >/dev/null || fail required_archive_member_missing
done
payload=$work/payload
mkdir -p "$payload/docs" "$payload/examples" "$payload/benchmarks"
: > "$payload/.manifest"
while IFS= read -r member; do
    (ulimit -f 20480; tar -xOf "$work/archive.tar" "$member" > "$payload/$member") 2>/dev/null || fail archive_member_extraction_failed
    chmod 600 "$payload/$member"
    printf '%s  %s\n' "$(hash_file "$payload/$member")" "$member" >> "$payload/.manifest"
done < "$work/members"
# Inspect a thin little-endian ELF64/Mach-O executable header without running it.
header=$(od -An -v -tx1 -N20 "$payload/tv-control" | tr -d ' \n')
[ "${#header}" -eq 40 ] || fail binary_header_truncated
case "$target" in
    *-linux-musl)
        [ "$(printf '%s' "$header" | cut -c1-12)" = 7f454c460201 ] || fail binary_format_mismatch
        executable_type=$(printf '%s' "$header" | cut -c33-36)
        case "$executable_type" in 0200|0300) ;; *) fail binary_format_mismatch ;; esac
        actual_machine=$(printf '%s' "$header" | cut -c37-40)
        case "$architecture:$actual_machine" in aarch64:b700|x86_64:3e00) ;; *) fail binary_architecture_mismatch ;; esac
        ;;
    *-apple-darwin)
        actual_machine=$(printf '%s' "$header" | cut -c1-16)
        [ "$actual_machine" = cffaedfe0c000001 ] || fail binary_architecture_mismatch
        [ "$(printf '%s' "$header" | cut -c25-32)" = 02000000 ] || fail binary_format_mismatch
        ;;
esac
chmod 700 "$payload/tv-control"
if [ -f "$payload/install.sh" ]; then chmod 700 "$payload/install.sh"; fi
printf '%s\n%s\n%s\n' "$version" "$target" "$expected" > "$payload/.receipt"
if [ ! -e "$prefix" ]; then
    new_root=$work/new-root
    mkdir -p "$new_root/versions"
    cat > "$new_root/tv-control" <<'LAUNCHER'
#!/bin/sh
set -eu
base=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
version=$(cat "$base/current")
case "$version" in v[0-9]*) ;; *) echo 'Invalid active version' >&2; exit 1 ;; esac
case "$version" in *[!A-Za-z0-9._-]*) echo 'Invalid active version' >&2; exit 1 ;; esac
exec "$base/versions/$version/tv-control" "$@"
LAUNCHER
    chmod 700 "$new_root/tv-control"
    printf 'TV_CONTROL_INSTALL_V1\n%s\n' "$(hash_file "$new_root/tv-control")" > "$new_root/.tv-control-owner"
    mv "$payload" "$new_root/versions/$version"
    printf '%s\n' "$version" > "$new_root/current"
    if ! { [ ! -e "$prefix" ] && [ ! -L "$prefix" ]; }; then fail publication_target_appeared; fi
    mv "$new_root" "$prefix" || fail installation_publication_failed
else
    if [ -e "$prefix/versions/$version" ] || [ -L "$prefix/versions/$version" ]; then
        if ! { [ -f "$prefix/versions/$version/.receipt" ] && [ "$(sed -n '3p' "$prefix/versions/$version/.receipt")" = "$expected" ]; }; then fail installed_version_conflict; fi
    else
        mv "$payload" "$prefix/versions/$version" || fail version_publication_failed
    fi
    activation=$(mktemp "$prefix/.current.XXXXXXXX") || fail activation_failed
    printf '%s\n' "$version" > "$activation"
    mv -f "$activation" "$prefix/current" || fail activation_failed
    activation=
fi
result installed 'Installed a verified release. No setup, device access or authentication was performed.'
if [ "$json" != yes ] && [ "$guided" != yes ]; then
    printf 'Executable: %s/tv-control\nAdd the installation directory to PATH yourself if desired; no shell startup file was changed.\n' "$prefix"
fi
if [ "$interactive" = yes ]; then
    if [ "$setup_requested" != yes ]; then
        printf 'Run guided setup now? It can query the LAN interface you choose; no pairing/auth is automatic. [y/N]: ' >&3
        if IFS= read -r answer <&3 && [ "$answer" = y ]; then setup_requested=yes; fi
    fi
    if [ "$setup_requested" = yes ]; then
        run_setup
    fi
fi
}
main "$@"
