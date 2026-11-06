# Running FreEco.ai on Podman — runbook

FreEco.ai and its services (CRM, Speaches, accounting, OmniRoute, the Claude
Code CLI) run as containers in one Linux VM, `podman-machine-default`, which
Podman runs in WSL on Windows. FreEco itself is the container `freeco-daemon`.
Everything below is typed in **Windows PowerShell**.

## What keeps it running

- Every container has a restart policy, and the VM runs Podman's restart
  service: when the VM starts, the containers start; when one crashes, it is
  started again.
- The VM boots the way Podman sets it up (`/root/bootstrap`). **Do not change
  `/etc/wsl.conf`** — in particular not `[boot] systemd = true`: then
  `podman machine start` never sees the VM as running, nothing keeps the VM
  alive, WSL stops it seconds after each command, and Windows' `podman`
  reports "dead network". That happened on 2026-10-22..25; the file was
  restored from `/etc/wsl.conf.bak-20261022`.
- The only thing that does not start by itself is the VM after Windows
  restarts. The **FreEco.ai** shortcut starts it, silently, then opens the
  window.

## Everyday

| I want to… | Command |
|---|---|
| Open FreEco | Shortcut **FreEco.ai** (or http://127.0.0.1:50051 in Brave) |
| See if FreEco is up | `curl.exe -s http://127.0.0.1:50051/api/health` |
| See all containers | `podman ps -a` |
| Read FreEco's log | `podman logs --tail 200 freeco-daemon` |
| Follow the log live | `podman logs -f freeco-daemon` (Ctrl+C to stop) |

## Wake (after sleep or a Windows restart)

```
podman machine start
```
Wait until `podman ps` lists `freeco-daemon`; FreEco answers within a minute.

If Podman says the VM "is not running" but `podman ps` works, Podman's own
record is out of date — the VM is fine. Use `wsl -d podman-machine-default`
for commands inside it, or `podman machine stop` then `podman machine start`
to bring the record back in line.

## Reload (FreEco misbehaves, the VM is fine)

```
podman restart freeco-daemon
```
One service only, e.g. Speaches: `podman restart freeco-speaches`.

## Reset the network (after sleep: "dead network", sites unreachable from containers)

```
podman machine stop
wsl --shutdown
podman machine start
```
`wsl --shutdown` stops every WSL distribution, so this restarts all
containers. Data is kept.

## Update FreEco to new code

From the repository, `C:\Users\freec\dev\freeco-ai-os`:
```
wsl -d podman-machine-default -u user -- sh -c "rm -rf ~/src/freeco-ai-os && mkdir -p ~/src/freeco-ai-os && cd /mnt/c/Users/freec/dev/freeco-ai-os && tar --exclude=./target --exclude=./.git -cf - . | (cd ~/src/freeco-ai-os && tar -xf -)"
podman machine ssh "cd ~/src/freeco-ai-os && sh deploy/daemon/check.sh"
podman machine ssh "cd ~/src/freeco-ai-os && podman build --memory=4g -t localhost/freeco-daemon:latest -f deploy/daemon/Containerfile ."
podman machine ssh "sh /home/user/freeco/run.sh"
```
Copying files goes through `wsl`; Podman commands inside the VM go through
`podman machine ssh` (a plain `wsl ... podman` fails with "failed to reexec").
`check.sh` runs fmt, clippy and all tests capped at 5 GB and ends with
CHECKS_OK or CHECKS_FAILED.
The build is capped at 4 GB so it cannot take the memory Windows needs; it
reuses earlier work, so an update usually takes minutes. Make a backup first
(Settings → Backups, or `POST /api/backups`).

## Reinstall the VM (last resort — the VM itself is broken)

Containers and their data live inside the VM. Back up first:
1. FreEco data: `wsl -d podman-machine-default -u user -- tar -czf /mnt/c/Users/freec/freeco-vm-backup.tgz -C /home/user freeco`
2. Service data: Settings → Backups → *Back up services*.

Then:
```
podman machine rm podman-machine-default
podman machine init --now
podman machine set --rootful=false
```
Restore `freeco-vm-backup.tgz` into `/home/user/`, rebuild the image (see
*Update*), start the services, and run `run.sh`.

## Memory

The VM takes up to about half of the computer's RAM and returns it slowly.
If Windows runs short (under 1 GB free), close what is not needed and stop
idle containers; a build or a test run is the usual cause, and both are
capped (`--memory=4g`, `--memory=5g`).
