# ──────────────────────────────────────────────────────────
#  p2pfiletransfer — launcher (Windows PowerShell)
#
#  Usage:
#    .\start.ps1                            Interactive mode
#    .\start.ps1 send .\myfile.txt          Send directly
#    .\start.ps1 receive /ip4/.../p2p/...   Receive directly
#
#  Options (append after file/address):
#    --relay <multiaddr>    Use relay for NAT traversal
#    --port <port>          Listen on specific port
# ──────────────────────────────────────────────────────────

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

# ── find binary ───────────────────────────────────────────

function Find-Binary {
    # 1. release build
    $rel = Join-Path $ScriptDir "target\release\p2pfiletransfer.exe"
    if (Test-Path $rel) { return $rel }

    # 2. debug build
    $dbg = Join-Path $ScriptDir "target\debug\p2pfiletransfer.exe"
    if (Test-Path $dbg) { return $dbg }

    # 3. in PATH
    $inPath = Get-Command p2pfiletransfer -ErrorAction SilentlyContinue
    if ($inPath) { return $inPath.Source }

    return $null
}

$Bin = Find-Binary
if (-not $Bin) {
    Write-Host "p2pfiletransfer binary not found."
    Write-Host "run .\setup.ps1 first to build the project."
    exit 1
}

# ── direct mode (arguments passed) ────────────────────────

if ($args.Count -ge 1) {
    switch ($args[0]) {
        "send" {
            if ($args.Count -lt 2) {
                Write-Host "usage: .\start.ps1 send <file-or-folder> [--relay ADDR] [--port PORT]"
                exit 1
            }
            $file = $args[1]
            $extra = @()
            if ($args.Count -gt 2) { $extra = $args[2..($args.Count - 1)] }
            & $Bin -m send -f $file @extra
            exit $LASTEXITCODE
        }
        { $_ -in "receive", "recv" } {
            if ($args.Count -lt 2) {
                Write-Host "usage: .\start.ps1 receive <sender-address> [--relay ADDR] [--port PORT]"
                exit 1
            }
            $addr = $args[1]
            $extra = @()
            if ($args.Count -gt 2) { $extra = $args[2..($args.Count - 1)] }
            & $Bin -m receive -a $addr @extra
            exit $LASTEXITCODE
        }
        { $_ -in "help", "-h", "--help" } {
            & $Bin --help
            exit 0
        }
        default {
            Write-Host "unknown command: $($args[0])"
            Write-Host "usage: .\start.ps1 [send|receive|help]"
            exit 1
        }
    }
}

# ── interactive mode ──────────────────────────────────────

Write-Host ""
Write-Host "==================================================="
Write-Host "  p2pfiletransfer"
Write-Host "==================================================="
Write-Host ""
Write-Host "  1) Send a file or folder"
Write-Host "  2) Receive a file or folder"
Write-Host "  3) Show help"
Write-Host "  4) Exit"
Write-Host ""

$choice = Read-Host "choose [1-4]"

switch ($choice) {
    "1" {
        Write-Host ""
        $filepath = Read-Host "path to file or folder"
        if (-not $filepath) { Write-Host "no path given."; exit 1 }
        if (-not (Test-Path $filepath)) { Write-Host "path not found: $filepath"; exit 1 }

        Write-Host ""
        $useRelay = Read-Host "use a relay server? [y/N]"
        $relayArgs = @()
        if ($useRelay -match '^[yY]') {
            $relayAddr = Read-Host "relay multiaddr"
            $relayArgs = @("-r", $relayAddr)
        }

        $port = Read-Host "listen port (enter for random)"
        $portArgs = @()
        if ($port) {
            $portArgs = @("-p", $port)
        }

        Write-Host ""
        Write-Host "starting sender..."
        Write-Host "───────────────────────────────────────────"
        & $Bin -m send -f $filepath @relayArgs @portArgs
        exit $LASTEXITCODE
    }
    "2" {
        Write-Host ""
        $address = Read-Host "sender address (/ip4/.../p2p/...)"
        if (-not $address) { Write-Host "no address given."; exit 1 }

        $useRelay = Read-Host "use a relay server? [y/N]"
        $relayArgs = @()
        if ($useRelay -match '^[yY]') {
            $relayAddr = Read-Host "relay multiaddr"
            $relayArgs = @("-r", $relayAddr)
        }

        $port = Read-Host "listen port (enter for random)"
        $portArgs = @()
        if ($port) {
            $portArgs = @("-p", $port)
        }

        Write-Host ""
        Write-Host "connecting to sender..."
        Write-Host "───────────────────────────────────────────"
        & $Bin -m receive -a $address @relayArgs @portArgs
        exit $LASTEXITCODE
    }
    "3" {
        & $Bin --help
        exit 0
    }
    "4" {
        Write-Host "bye."
        exit 0
    }
    default {
        Write-Host "invalid choice."
        exit 1
    }
}
