# ──────────────────────────────────────────────────────────
#  p2pfiletransfer — setup script (Windows PowerShell)
# ──────────────────────────────────────────────────────────

$ErrorActionPreference = "Stop"

function Info  { param($msg) Write-Host "[info]  $msg" -ForegroundColor Cyan }
function Ok    { param($msg) Write-Host "[ok]    $msg" -ForegroundColor Green }
function Warn  { param($msg) Write-Host "[warn]  $msg" -ForegroundColor Yellow }
function Err   { param($msg) Write-Host "[error] $msg" -ForegroundColor Red; exit 1 }

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptDir

Write-Host ""
Write-Host "==================================================="
Write-Host "  p2pfiletransfer setup (Windows)"
Write-Host "==================================================="
Write-Host ""

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
Info "detected: Windows ($Arch)"

# ── check Visual Studio Build Tools ───────────────────────

function Check-BuildTools {
    Info "checking C++ build tools..."

    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vsWhere) {
        $installations = & $vsWhere -latest -property installationPath 2>$null
        if ($installations) {
            Ok "Visual Studio / Build Tools found"
            return
        }
    }

    # check if cl.exe is reachable
    $cl = Get-Command cl.exe -ErrorAction SilentlyContinue
    if ($cl) {
        Ok "C++ compiler found: $($cl.Source)"
        return
    }

    Warn "Visual Studio Build Tools (C++ workload) not detected"
    Warn "download from: https://visualstudio.microsoft.com/visual-cpp-build-tools/"
    Write-Host ""
    $answer = Read-Host "Continue anyway? [y/N]"
    if ($answer -notmatch '^[yY]') {
        Err "install Build Tools first, then rerun this script"
    }
}

# ── install Rust ──────────────────────────────────────────

function Install-Rust {
    $rustc = Get-Command rustc -ErrorAction SilentlyContinue
    if ($rustc) {
        $ver = & rustc --version
        Ok "rust already installed: $ver"
        return
    }

    Info "rust not found — downloading rustup-init.exe..."

    $installer = "$env:TEMP\rustup-init.exe"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $installer -UseBasicParsing

    Info "installing rust (stable)..."
    & $installer -y --default-toolchain stable
    Remove-Item $installer -ErrorAction SilentlyContinue

    # refresh PATH
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

    $rustc = Get-Command rustc -ErrorAction SilentlyContinue
    if ($rustc) {
        Ok "rust installed: $(& rustc --version)"
    } else {
        Err "rust installation failed — visit https://rustup.rs"
    }
}

# ── build ─────────────────────────────────────────────────

function Build-Project {
    Info "building p2pfiletransfer (release mode)..."

    # ensure cargo is in PATH
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    }

    & cargo build --release
    if ($LASTEXITCODE -ne 0) {
        Err "build failed"
    }

    $binary = Join-Path $ScriptDir "target\release\p2pfiletransfer.exe"
    if (Test-Path $binary) {
        Ok "build complete: target\release\p2pfiletransfer.exe"
    } else {
        Err "build failed — binary not found"
    }
}

# ── install to PATH (optional) ────────────────────────────

function Install-Binary {
    $dest = "$env:USERPROFILE\.cargo\bin"

    Write-Host ""
    $answer = Read-Host "Install to $dest so you can run 'p2pfiletransfer' from anywhere? [y/N]"
    if ($answer -match '^[yY]') {
        if (-not (Test-Path $dest)) { New-Item -ItemType Directory -Path $dest | Out-Null }

        Copy-Item "$ScriptDir\target\release\p2pfiletransfer.exe" "$dest\p2pfiletransfer.exe" -Force
        Ok "installed to $dest"

        if ($env:PATH -notlike "*$dest*") {
            Warn "add $dest to your system PATH if not already there"
        }
    } else {
        Info "skipped — run directly with: .\target\release\p2pfiletransfer.exe"
    }
}

# ── run ───────────────────────────────────────────────────

Check-BuildTools
Install-Rust
Build-Project
Install-Binary

Write-Host ""
Write-Host "==================================================="
Write-Host "  setup complete!"
Write-Host ""
Write-Host "  quick start:"
Write-Host "    .\start.ps1 send .\myfile.txt"
Write-Host "    .\start.ps1 receive /ip4/.../p2p/..."
Write-Host "==================================================="
Write-Host ""
