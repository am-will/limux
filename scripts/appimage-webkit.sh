#!/usr/bin/env bash

resolve_webkitgtk_runtime_dir() {
    local libdir
    local candidate

    if command -v pkg-config >/dev/null 2>&1; then
        libdir="$(pkg-config --variable=libdir webkitgtk-6.0 2>/dev/null || true)"
        if [ -n "$libdir" ]; then
            for candidate in \
                "${libdir}/webkitgtk-6.0" \
                "${libdir}/$(uname -m)-linux-gnu/webkitgtk-6.0"
            do
                if [ -d "$candidate" ]; then
                    printf '%s\n' "$candidate"
                    return 0
                fi
            done
        fi
    fi

    for candidate in \
        "/usr/lib/$(uname -m)-linux-gnu/webkitgtk-6.0" \
        "/usr/lib/x86_64-linux-gnu/webkitgtk-6.0" \
        "/usr/lib/aarch64-linux-gnu/webkitgtk-6.0" \
        "/usr/lib64/webkitgtk-6.0" \
        "/usr/lib/webkitgtk-6.0"
    do
        if [ -d "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

resolve_webkitgtk_process_dir() {
    local prefix
    local candidate

    if [ -n "$WEBKITGTK_RUNTIME_DIR" ] && [ -x "${WEBKITGTK_RUNTIME_DIR}/WebKitWebProcess" ]; then
        printf '%s\n' "$WEBKITGTK_RUNTIME_DIR"
        return 0
    fi

    if command -v pkg-config >/dev/null 2>&1; then
        prefix="$(pkg-config --variable=prefix webkitgtk-6.0 2>/dev/null || true)"
        if [ -n "$prefix" ]; then
            for candidate in \
                "${prefix}/libexec/webkitgtk-6.0" \
                "${prefix}/lib/webkitgtk-6.0" \
                "${prefix}/lib64/webkitgtk-6.0"
            do
                if [ -x "${candidate}/WebKitWebProcess" ]; then
                    printf '%s\n' "$candidate"
                    return 0
                fi
            done
        fi
    fi

    for candidate in \
        "/usr/libexec/webkitgtk-6.0" \
        "/usr/lib/$(uname -m)-linux-gnu/webkitgtk-6.0" \
        "/usr/lib/x86_64-linux-gnu/webkitgtk-6.0" \
        "/usr/lib/aarch64-linux-gnu/webkitgtk-6.0" \
        "/usr/lib64/webkitgtk-6.0" \
        "/usr/lib/webkitgtk-6.0"
    do
        if [ -x "${candidate}/WebKitWebProcess" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done

    return 1
}

is_appimage_system_library() {
    local path="$1"
    local base

    base="$(basename "$path")"

    case "$base" in
        ld-linux*.so*|libanl.so*|libBrokenLocale.so*|libc.so*|libcidn.so*|libdl.so*|libm.so*|libmvec.so*|libnsl.so*|libnss_*.so*|libpthread.so*|libresolv.so*|librt.so*|libthread_db.so*|libutil.so*)
            return 0
            ;;
        libEGL.so*|libGL.so*|libGLESv1_CM.so*|libGLESv2.so*|libGLX.so*|libGLdispatch.so*|libOpenGL.so*|libdrm*.so*|libgbm.so*|libglapi.so*|libwayland-client.so*|libwayland-cursor.so*|libwayland-egl.so*|libwayland-server.so*)
            return 0
            ;;
    esac

    return 1
}

shared_library_dependencies() {
    local path="$1"

    ldd "$path" 2>/dev/null \
        | awk '
            /=> \// { print $3; next }
            /^\// { print $1; next }
        ' \
        | sort -u
}

binary_replace_string() {
    local file="$1"
    local old="$2"
    local new="$3"

    if [ "${#new}" -gt "${#old}" ]; then
        echo "ERROR: replacement string is longer than original while patching ${file}"
        echo "  original:    ${old}"
        echo "  replacement: ${new}"
        exit 1
    fi

    python3 - "$file" "$old" "$new" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
old = sys.argv[2].encode()
new = sys.argv[3].encode()

data = path.read_bytes()
replacement = new + (b"\0" * (len(old) - len(new)))
patched = data.replace(old, replacement)
if patched != data:
    path.write_bytes(patched)
PY
}

copy_appimage_library_closure() {
    local dest_dir="$1"
    shift

    local -a queue=("$@")
    local -A copied=()
    local -A processed=()
    local source
    local dep
    local resolved
    local target

    mkdir -p "$dest_dir"

    while [ "${#queue[@]}" -gt 0 ]; do
        source="${queue[0]}"
        queue=("${queue[@]:1}")

        if [ ! -e "$source" ] || [ -n "${processed[$source]:-}" ]; then
            continue
        fi
        processed["$source"]=1

        while IFS= read -r dep; do
            if [ -z "$dep" ] || [ ! -e "$dep" ] || is_appimage_system_library "$dep"; then
                continue
            fi

            resolved="$(readlink -f "$dep")"
            target="${dest_dir}/$(basename "$dep")"
            if [ -z "${copied[$resolved]:-}" ] && [ ! -e "$target" ]; then
                cp -L "$dep" "$target"
                chmod 755 "$target"
                strip --strip-debug "$target" 2>/dev/null || true
                assert_glibc_compatibility "$target" "AppImage dependency $(basename "$target")"
            fi
            copied["$resolved"]=1

            queue+=("$resolved")
        done < <(shared_library_dependencies "$source")
    done
}

patch_appimage_webkit_paths() {
    local appdir="$1"
    local relative_runtime="usr/lib/webkitgtk-6.0/"
    local relative_bundle="usr/lib/webkitgtk-6.0/injected-bundle/"
    local file

    while IFS= read -r file; do
        # Distro WebKitGTK embeds absolute paths and does not reliably honor
        # WEBKIT_EXEC_PATH outside developer builds.
        binary_replace_string "$file" "${WEBKITGTK_RUNTIME_DIR%/}/injected-bundle/" "$relative_bundle"
        binary_replace_string "$file" "${WEBKITGTK_PROCESS_DIR%/}/" "$relative_runtime"
        binary_replace_string "$file" "${WEBKITGTK_RUNTIME_DIR%/}/" "$relative_runtime"
    done < <(find "$appdir/usr/lib" -type f \( -perm -0100 -o -name '*.so*' \) | sort)
}

copy_appimage_webkit_runtime() {
    local appdir="$1"
    local runtime_dest="$appdir/usr/lib/webkitgtk-6.0"
    local -a runtime_binaries=()
    local -a app_binaries=()
    local entry

    if [ -z "$WEBKITGTK_RUNTIME_DIR" ]; then
        echo "ERROR: WebKitGTK runtime directory was not resolved."
        exit 1
    fi

    mkdir -p "$(dirname "$runtime_dest")"
    cp -a "$WEBKITGTK_RUNTIME_DIR" "$runtime_dest"
    if [ "$WEBKITGTK_PROCESS_DIR" != "$WEBKITGTK_RUNTIME_DIR" ]; then
        cp -a "$WEBKITGTK_PROCESS_DIR"/. "$runtime_dest/"
    fi

    while IFS= read -r entry; do
        runtime_binaries+=("$entry")
    done < <(find "$runtime_dest" -type f \( -perm -0100 -o -name '*.so*' \) | sort)

    if [ -n "${CLI_BINARY:-}" ]; then
        app_binaries+=("$CLI_BINARY")
    fi
    if [ -n "${HOST_BINARY:-}" ]; then
        app_binaries+=("$HOST_BINARY")
    elif [ -n "${BINARY:-}" ]; then
        app_binaries+=("$BINARY")
    fi

    copy_appimage_library_closure "$appdir/usr/lib" "${app_binaries[@]}" "$GHOSTTY_SO" "${runtime_binaries[@]}"
    patch_appimage_webkit_paths "$appdir"
}
