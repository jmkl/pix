#!/usr/bin/env nu
# Build Pix and install it for the current user (no admin needed).
#
#   nu packaging/install.nu                  build + install
#   nu packaging/install.nu --no-build       install an existing release build
#   nu packaging/install.nu --add-to-path    (Windows) also add it to your PATH
#   nu packaging/install.nu --uninstall      remove it again
#
# Windows: pix.exe goes to %LOCALAPPDATA%\Programs\Pix and the image file
#          associations are registered (HKCU).
# Linux:   ~/.local/bin/pix plus a desktop entry.
# macOS:   ~/.local/bin/pix only.

def main [
    --prefix: path     # install directory (default: a per-user location)
    --no-build         # skip `cargo build --release`
    --no-register      # Windows: do not register file associations
    --add-to-path      # Windows: add the install directory to your user PATH
    --uninstall        # remove a previous installation
] {
    let os = $nu.os-info.name
    let root = ($env.FILE_PWD | path dirname)
    let exe_name = if $os == "windows" { "pix.exe" } else { "pix" }
    let dest_dir = if $prefix != null {
        $prefix
    } else if $os == "windows" {
        $env.LOCALAPPDATA | path join "Programs" "Pix"
    } else {
        $nu.home-dir | path join ".local" "bin"
    }
    let dest_exe = ($dest_dir | path join $exe_name)
    let desktop_entry = ($nu.home-dir | path join ".local" "share" "applications" "pix.desktop")

    if $uninstall {
        if $os == "windows" {
            let unreg = ($dest_dir | path join "unregister.ps1")
            if ($unreg | path exists) {
                ^powershell -NoProfile -ExecutionPolicy Bypass -File $unreg
            }
            win-user-path "remove" $dest_dir
            # Only remove what we installed; the directory may be shared.
            for name in [$exe_name "register.ps1" "unregister.ps1"] {
                rm --force ($dest_dir | path join $name)
            }
            if ($dest_dir | path exists) and (ls --all $dest_dir | is-empty) {
                rm $dest_dir
            }
        } else {
            rm --force $dest_exe
            if $os == "linux" { rm --force $desktop_entry }
        }
        print "Pix has been uninstalled."
        return
    }

    # 1. Build
    if not $no_build {
        print "Building (cargo build --release)..."
        ^cargo build --release --manifest-path ($root | path join "Cargo.toml")
        check "cargo build" $env.LAST_EXIT_CODE
    }
    let built = ($root | path join "target" "release" $exe_name)
    if not ($built | path exists) {
        error make { msg: $"($built) not found. Run without --no-build, or build it first." }
    }

    # 2. Copy the binary
    mkdir $dest_dir
    cp $built $dest_exe
    print $"Installed ($dest_exe)"

    # 3. Platform integration
    if $os == "windows" {
        # Keep the scripts next to the exe so they can be run later.
        for name in ["register.ps1" "unregister.ps1"] {
            cp ($root | path join "packaging" "windows" $name) $dest_dir
        }
        if not $no_register {
            ^powershell -NoProfile -ExecutionPolicy Bypass -File ($dest_dir | path join "register.ps1") -Exe $dest_exe
            check "file association registration" $env.LAST_EXIT_CODE
        }
        if $add_to_path {
            win-user-path "add" $dest_dir
            print "Added to your user PATH (open a new terminal to pick it up)."
        }
    } else if $os == "linux" {
        let apps_dir = ($desktop_entry | path dirname)
        mkdir $apps_dir
        let entry = (
            open --raw ($root | path join "packaging" "linux" "pix.desktop")
            | into string
            | str replace "Exec=pix %f" $"Exec=($dest_exe) %f"
        )
        $entry | save --force $desktop_entry
        if (which update-desktop-database | is-not-empty) {
            ^update-desktop-database $apps_dir
        }
        print $"Desktop entry written to ($desktop_entry)"
    }

    if $os != "windows" and not ($env.PATH | any {|p| $p == $dest_dir }) {
        print $"Note: ($dest_dir) is not on your PATH."
    }

    print "Done."
    if $os == "windows" and not $no_register {
        print "Finish in Settings > Apps > Default apps > Pix to choose it for image types."
    }
}

# Fail with a clear message when an external command exited non-zero.
def check [what: string, code: int] {
    if $code != 0 {
        error make { msg: $"($what) failed with exit code ($code)" }
    }
}

# Add or remove `dir` in the Windows *user* PATH (idempotent).
def win-user-path [action: string, dir: string] {
    let script = r#'
$dir = $env:PIX_DIR
$current = [Environment]::GetEnvironmentVariable('Path', 'User')
$parts = @($current -split ';' | Where-Object { $_ -and $_ -ne $dir })
if ($env:PIX_ACTION -eq 'add') { $parts += $dir }
[Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
'#
    with-env { PIX_DIR: $dir, PIX_ACTION: $action } {
        ^powershell -NoProfile -Command $script
    }
}
