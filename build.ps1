# Builds both launchers in release mode and packs them into dist/: the SJK Launcher
# and the JoF Launcher (-features jof, built in target/jof so neither rebuilds the
# other).
#   ./build.ps1          build and pack
#   ./build.ps1 -Test    run the tests first (both looks)
param([switch]$Test)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

if ($Test) {
    cargo test --locked --workspace
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    cargo test --locked -p sjk-launcher --features jof --target-dir target/jof
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }

function Pack([string]$program, [string]$target, [string[]]$flags) {
    cargo build --locked --release -p sjk-launcher --target-dir $target @flags
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
    $name = "$program-windows-$arch"
    $stage = "dist/$name"
    if (Get-Process $program -ErrorAction SilentlyContinue | Where-Object Path -like "$PSScriptRoot\dist\*") {
        Write-Error "Close the $program running from dist\ first; Windows locks its exe."
    }
    Remove-Item -Recurse -Force $stage, "dist/$name.zip" -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    # The fonts are built into the exe; their licence asks to travel with them.
    Copy-Item "$target/release/sjk-launcher.exe" "$stage/$program.exe"
    Copy-Item assets/fonts/*-OFL.txt $stage
    Compress-Archive "$stage/*" "dist/$name.zip"
    Write-Host "Built dist/$name.zip"
}

Pack sjk-launcher target @()
Pack jof-launcher target/jof @('--features', 'jof')
