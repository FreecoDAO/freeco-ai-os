# Move FreEco.ai's data from the Windows home into the Podman VM, for the
# daemon container (deploy/daemon). Run on Windows, with the Windows daemon
# stopped so the database is not copied mid-write.
#
# Copies only what the daemon uses (about 100 MB): configuration, agents,
# database, workspaces, hands, schedules, the vault and its key file. The
# Windows home is left exactly as it was -- it stays the fallback -- and
# backups, local models and installers are not copied.
param(
    [string]$WinHome = (Join-Path $env:USERPROFILE '.freeco-ai'),
    [string]$VmHome = '/home/user/freeco'
)
$ErrorActionPreference = 'Stop'
$podman = (Get-Command podman -ErrorAction SilentlyContinue).Source
if (-not $podman) {
    $podman = (Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Programs\podman') -Recurse -Filter podman.exe | Select-Object -First 1).FullName
}
function Vm([string]$cmd) {
    # Straight through WSL: `podman machine ssh` refuses whenever Podman's own
    # record says the VM is stopped, which it can while the VM runs fine.
    & wsl.exe -d podman-machine-default -u user -- sh -c $cmd
    if ($LASTEXITCODE -ne 0) { throw "VM command failed: $cmd" }
}
function ToVm([string]$winPath) {
    '/mnt/' + $winPath.Substring(0,1).ToLower() + ($winPath.Substring(2) -replace '\\','/')
}

try {
    $health = Invoke-WebRequest -UseBasicParsing http://127.0.0.1:50051/api/health -TimeoutSec 3
    throw "FreEco is still running (HTTP $($health.StatusCode)). Stop it first, so the database is copied whole."
} catch [System.Net.WebException] { }

$src = ToVm $WinHome
$items = @('config.toml','agents','data','workspaces','hands','hand_state.json','cron_jobs.json',
           'custom_models.json','served_models.json','secrets.env','vault.enc','skills','workflows',
           'services','archive','agents-archived','omniroute.env','.dashboard_setup_dismissed')
Vm "mkdir -p $VmHome/xdg/freeco $VmHome/logs"
foreach ($i in $items) {
    if (Test-Path (Join-Path $WinHome $i)) {
        Write-Host "  copying $i"
        Vm "cp -a '$src/$i' '$VmHome/'"
    }
}

# The WhatsApp login (Baileys multi-file auth), so WhatsApp stays linked
# without scanning the QR code again. Only the login: the kernel re-extracts
# the gateway's code and runs `npm install` itself, and the Windows
# node_modules is not usable on Linux. Copy it only with nothing else logged
# in on it -- two gateways on one login knock each other off ("conflict:
# replaced") and duplicate messages.
$wa = Join-Path $WinHome 'whatsapp-gateway\auth_store'
if (Test-Path $wa) {
    Write-Host '  copying whatsapp-gateway/auth_store'
    Vm "mkdir -p '$VmHome/whatsapp-gateway' && cp -a '$(ToVm $wa)' '$VmHome/whatsapp-gateway/'"
}

# The vault's key file. It is masked with the user and computer name, which
# the container is given below, so the same vault opens unchanged and the key
# itself is never written out anywhere.
$keyring = Join-Path $env:LOCALAPPDATA 'freeco\.keyring'
if (Test-Path $keyring) {
    Vm "cp '$(ToVm $keyring)' '$VmHome/xdg/freeco/.keyring' && chmod 600 '$VmHome/xdg/freeco/.keyring'"
}

# Container environment: the names the vault key is masked with.
$envFile = "USERNAME=$env:USERNAME`nCOMPUTERNAME=$env:COMPUTERNAME`nTZ=Europe/Zurich`n"
$tmp = Join-Path $env:TEMP 'freeco-container.env'
[IO.File]::WriteAllText($tmp, $envFile)
Vm "cp '$(ToVm $tmp)' '$VmHome/container.env' && chmod 600 '$VmHome/container.env'"
Remove-Item $tmp

# The container has no screen: the browser hand runs headless there.
Vm "sed -i 's/^headless = false/headless = true/' '$VmHome/config.toml'"

# Agent settings keep their Windows paths here; the daemon re-roots any path
# under the old .freeco-ai home into this one when it starts
# (`remap_home_path`), so the files are not edited.

Vm "cp '$(ToVm (Join-Path $PSScriptRoot 'run.sh'))' '$VmHome/run.sh'"
Write-Host "Data is in the VM at $VmHome. Start the container with:"
Write-Host "  podman machine ssh 'sh $VmHome/run.sh'"
