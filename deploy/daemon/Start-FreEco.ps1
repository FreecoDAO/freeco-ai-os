# Opens FreEco.ai, starting what it needs. For the daemon container
# (deploy/daemon): FreEco runs inside the Podman VM and restarts itself, so
# this only makes sure the VM is up and then opens the window.
#
# Run hidden from the shortcut:
#   conhost.exe --headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File Start-FreEco.ps1 [-Widget]
# `conhost --headless` shows no window at all; with Windows Terminal as the
# default console, `-WindowStyle Hidden` still opened one, showing restored
# old command-prompt tabs that looked like FreEco had failed.
param([switch]$Widget)

Add-Type -AssemblyName System.Windows.Forms
function Fail([string]$text) {
    [System.Windows.Forms.MessageBox]::Show($text, 'FreEco.ai', 'OK', 'Error') | Out-Null
    exit 1
}
function Healthy {
    try { (Invoke-WebRequest -UseBasicParsing http://127.0.0.1:50051/api/health -TimeoutSec 3).StatusCode -eq 200 }
    catch { $false }
}

if (-not (Healthy)) {
    $podman = (Get-Command podman -ErrorAction SilentlyContinue).Source
    if (-not $podman) {
        $hit = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Programs\podman') -Recurse -Filter podman.exe -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($hit) { $podman = $hit.FullName }
    }
    if (-not $podman) { Fail 'Podman is not installed, and FreEco runs in a Podman container.' }

    # The VM. Starting it starts the FreEco container too (restart policy).
    & $podman ps -q 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { & $podman machine start 2>&1 | Out-Null }

    $ok = $false
    for ($i = 0; $i -lt 60; $i++) {
        if (Healthy) { $ok = $true; break }
        if ($i -eq 20) { & $podman start freeco-daemon 2>&1 | Out-Null }
        Start-Sleep -Seconds 2
    }
    if (-not $ok) {
        Fail "FreEco did not answer within two minutes.`n`nIts log: podman logs freeco-daemon"
    }
}

$url = 'http://127.0.0.1:50051/'
$extra = @()
if ($Widget) { $url += '?widget=1'; $extra += '--window-size=440,720' }
$brave = Join-Path $env:ProgramFiles 'BraveSoftware\Brave-Browser\Application\brave.exe'
if (Test-Path $brave) { Start-Process $brave -ArgumentList (@("--app=$url") + $extra) }
else { Start-Process $url }
