# Containers without Docker Desktop

Status: design, work in progress.

Goal: run Docker containers and Compose projects from Brisk on Windows without installing
Docker Desktop or WSL, and without administrator rights in daily use.

## Constraints that shape the design

- Linux containers need a Linux kernel, so a small Linux virtual machine is unavoidable.
- Windows Hypervisor Platform (WHP, `WinHvPlatform.dll`) lets an ordinary, non-elevated
  process create and run a VM. Verified on Windows 11 26300: `WHvCreatePartition`,
  `WHvSetupPartition` and `WHvDeletePartition` succeed unelevated. Administrator rights are
  needed once, to turn the Windows feature on.
- Hyper-V's Host Compute Service (what WSL 2 uses) needs elevation or a privileged service
  for every VM, so it is not the first backend.
- WHP exists on Windows 10/11 Home and Pro. Full Hyper-V is not needed.

## Architecture

```
Brisk (brisk-vm)                                   Linux VM ("Brisk Engine")
-----------------                                  -------------------------
engine manager ── starts ──> qemu-system-x86_64 -accel whpx (headless)
                              kernel + initramfs (direct boot), read-only base image,
                              sparse data disk for /var/lib/docker
control mux  <── named pipe ─ virtio-serial ──>    brisk-agent (Rust, static)
  ├─ Docker API stream                               ├─ dockerd + containerd
  ├─ exec / logs streams                             ├─ port watcher (docker events)
  ├─ port-forward streams                            ├─ file sync receiver (inotify back)
  └─ file sync                                       └─ memory stats, balloon
127.0.0.1:<published port> listeners  <── mux ──>  guest 127.0.0.1:<port>
\\.\pipe\brisk-docker (Docker API for CLI tools)
```

- **VM.** QEMU with the WHPX accelerator, no display, 2 vCPUs and up to 2 GB by default.
  The kernel and initramfs boot directly (no bootloader); the base system is a read-only
  image; images and volumes live on a sparse data disk that grows on demand (64 GB limit).
  Target: cold start under 3 s, idle under 150 MB of host memory (virtio-balloon with free
  page reporting gives unused guest memory back to Windows).
- **One channel between host and guest.** A virtio-serial port backed by a named pipe that
  only the current user can open. Everything is multiplexed over it: RPC, the Docker API,
  exec and log streams, port forwards and file sync. No TCP port is opened for control.
- **brisk-agent** runs in the guest: starts dockerd, watches Docker events, forwards
  connections, applies file changes and reports memory use.
- **Docker API.** Brisk talks the Docker Engine API (HTTP over a mux stream) for its UI;
  no `docker.exe` is needed. For other tools Brisk also serves `\\.\pipe\brisk-docker` and
  sets `DOCKER_HOST=npipe:////./pipe/brisk-docker` in its terminal, so an installed
  `docker` / `docker compose` CLI works against the engine.

## Networking

- **Internet** from containers through QEMU's user-mode network (NAT, DNS); nothing to
  configure, no virtual switch, no administrator rights.
- **Published ports** (`-p 8000:8000`, `ports:` in Compose) appear on Windows at the same
  number on `127.0.0.1`: the agent reports them from Docker events, Brisk opens a listener
  and forwards each connection through the mux. `localhost:8000` in a browser just works.
  A port that is busy on Windows is reported with the process holding it.
- **Host from containers**: `host.docker.internal` resolves to the host (QEMU's 10.0.2.2),
  so a container reaches a database running on Windows.
- **Between containers**: Docker's own networks inside the VM, unchanged.
- Listening on all interfaces (to reach a container from another machine) is an explicit
  setting; the default is loopback only.

## Files

Bind mounts are the hard part: QEMU on Windows has no fast shared file system, and Docker
Desktop's shared folders are known to be slow. Brisk syncs instead:

- The project folder is mirrored into the VM at a fixed path. A bind mount of a path inside
  the project (`.:/app` in Compose) maps to the mirror.
- Host to guest: Brisk already watches the project; a change reaches the VM within tens of
  milliseconds, so dev servers with auto-reload work.
- Guest to host: the agent watches the mirror with inotify, so files a container writes
  (migrations, generated code, coverage) come back.
- Excluded by default: `.git` internals, `.venv`, `node_modules`, `__pycache__` (Windows
  environments are useless in Linux). Configurable per project.
- A bind mount outside the project is mirrored on demand with a warning.

## User experience

- **First use** (opening a `docker-compose.yml`, or the Docker node in Services) shows a
  setup card instead of an error:
  1. Virtualization check. If Windows Hypervisor Platform is off: "Turn on" runs
     `dism /online /enable-feature /featurename:HypervisorPlatform /all /norestart` through
     one UAC prompt, then offers Restart now / Later. If virtualization is off in the
     firmware, it shows how to turn it on (this cannot be done from Windows).
  2. Engine download, with its size, progress and SHA-256 check, into
     `%LOCALAPPDATA%\Brisk\engine` (not in the IDE zip, which stays small).
  3. Start, with boot progress.
- **Services → Docker**:
  - engine state with CPU and memory use, Start / Stop, settings (CPUs, memory, disk,
    auto-stop);
  - Compose projects of the open project with their services, state, ports as links,
    Up / Down / Restart / Rebuild;
  - Containers, Images, Volumes, Networks with filter and actions (Logs, Terminal, Inspect,
    Restart, Stop, Remove, Open in Browser);
  - a detail pane: Logs (follow, search, colors), Terminal (exec into the container in
    Brisk's terminal), Inspect, Environment, Ports, Files.
- **Editor**: run marks in `docker-compose.yml` (per service and for the whole file) and in
  a `Dockerfile` (Build, Run). Run configurations of kind Docker Compose and Dockerfile.
- **Python**: a Compose service or an image as the project interpreter, with debugging
  through a forwarded debugpy port (shared with the remote interpreters work).
- **Lifecycle**: the engine starts on the first Docker action and stops itself after a
  configurable idle time without running containers.

## Distribution and licenses

The engine is a separate download, published as a release asset next to the IDE zip:
QEMU for Windows (GPL-2.0, unmodified, with a link to its source), the guest kernel and
base image (Alpine Linux packages under their own licenses), Docker Engine and containerd
(Apache-2.0), and brisk-agent (Brisk's license). It carries its own notices file.

## Phases

| Phase | Content | Estimate |
|---|---|---|
| 0 | `brisk-vm` crate: virtualization check, one-click enable, setup card | days |
| 1 | Engine bundle (built in CI), QEMU launch, agent over virtio-serial, `docker info` round trip; boot time and memory measured | 2–3 weeks |
| 2 | Docker API client; Services → Docker: lists, start / stop / remove, logs, exec terminal | 2 weeks |
| 3 | Automatic port forwarding, `host.docker.internal` | 1 week |
| 4 | File sync and bind mounts; Compose up / down / logs; run marks | 3–4 weeks |
| 5 | Python interpreter in a container or Compose service, debugging | 2 weeks |
| 6 | Auto-stop, balloon, settings, `\\.\pipe\brisk-docker` for CLI tools | 1–2 weeks |

## Risks

- QEMU's WHPX accelerator is less tuned than KVM. Fallback: a Host Compute Service backend
  behind a one-time installed service.
- File sync must never lose a change; it needs its own test suite (renames, deletes,
  editors that save through temporary files, both sides editing).
- Antivirus scanning of the data disk, corporate machines with virtualization disabled by
  policy.
