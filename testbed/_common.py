"""
Shared helpers for the weverywhere VM testbed.

Both testbed scripts import this module:

  * create-test-vm-images.py  - builds each VM's qcow2 by running an unattended
    install under qemu (Windows autounattend / Linux cloud-init). It uses qemu
    user-mode networking, so it never touches host network interfaces.

  * run-test-vm-network.py    - brings up a host bridge with per-VM tap devices,
    boots every VM with its hard-coded static IP, and tears every interface it
    created back down on exit.

Design notes
------------
* Storage: everything lives under ./testbed/. Shared downloads (cloud images,
  the Windows ISO, OpenSSH) go in testbed/cache/; per-VM artifacts (disk, seed
  ISO, SSH keypair, build manifest, serial log) go in testbed/vms/<hostname>/.

* Rebuild-on-change: a VM is rebuilt only when its build manifest hash changes.
  The hash covers the VM's config entry, the global settings it depends on, the
  base image identity, and the weverywhere binary it bakes in - so editing
  vm_config.py (or shipping a new binary) triggers exactly the rebuilds needed.

* Networking is split so teardown is trivially correct: the install phase uses
  qemu user-mode networking (zero host interfaces), and only the run phase
  creates a bridge + taps, which are always removed on exit.
"""

from __future__ import annotations

import atexit
import contextlib
import hashlib
import ipaddress
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
from io import BytesIO
from pathlib import Path

import vm_config

# --------------------------------------------------------------------------
# Paths
# --------------------------------------------------------------------------

TESTBED_DIR = Path(__file__).resolve().parent
REPO_ROOT = TESTBED_DIR.parent
CACHE_DIR = TESTBED_DIR / "cache"
VMS_DIR = TESTBED_DIR / "vms"
DIST_DIR = REPO_ROOT / "dist"


def _resolve_weverywhere_version() -> str:
    """Reuse the repo's single source of truth for the version string."""
    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    from _version import resolve_version  # noqa: E402

    return resolve_version()


WEVERYWHERE_VERSION = _resolve_weverywhere_version()


# --------------------------------------------------------------------------
# Small utilities
# --------------------------------------------------------------------------

def log(msg: str) -> None:
    print(f"[testbed] {msg}", flush=True)


def run(cmd: list[str], *, sudo: bool = False, check: bool = True,
        capture: bool = False, cwd: Path | None = None) -> subprocess.CompletedProcess:
    if sudo and os.geteuid() != 0:
        cmd = ["sudo", *cmd]
    return subprocess.run(
        cmd, check=check,
        capture_output=capture, text=True,
        cwd=str(cwd) if cwd else None,
    )


def human_bytes(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            return f"{n:.0f}{unit}" if unit == "B" else f"{n:.1f}{unit}"
        n /= 1024
    return f"{n:.1f}TB"


def is_windows(vm: dict) -> bool:
    return vm["os"].lower() == "windows"


def vm_dir(vm: dict) -> Path:
    d = VMS_DIR / vm["hostname"]
    d.mkdir(parents=True, exist_ok=True)
    return d


def all_vms() -> list[dict]:
    return list(vm_config.VMS)


def select_vms(names: list[str]) -> list[dict]:
    """Return the VMs whose hostname is in `names` (or all if empty)."""
    if not names:
        return all_vms()
    by_name = {vm["hostname"]: vm for vm in all_vms()}
    unknown = [n for n in names if n not in by_name]
    if unknown:
        sys.exit(f"[testbed] unknown VM(s): {', '.join(unknown)}\n"
                 f"          known: {', '.join(by_name)}")
    return [by_name[n] for n in names]


def bootup_binary_path(vm: dict) -> str:
    """The in-guest path of the weverywhere binary, with {VERSION} filled in."""
    return vm["bootup-binary"].replace("{VERSION}", WEVERYWHERE_VERSION)


def weverywhere_platform(vm: dict) -> str:
    """The scripts/build.py target name for the binary this VM needs."""
    return "windows-x64" if is_windows(vm) else "linux-x64"


def host_weverywhere_binary(vm: dict) -> Path:
    """The freshly built host-side artifact that gets baked into this VM."""
    plat = weverywhere_platform(vm)
    suffix = ".exe" if is_windows(vm) else ""
    name = f"weverywhere-{WEVERYWHERE_VERSION}-{plat}{suffix}"
    return DIST_DIR / plat / name


def ensure_weverywhere_binary(vm: dict) -> Path:
    """
    Return the weverywhere binary this VM bakes in, building it on demand.

    If the versioned artifact is not already staged under dist/, run
    scripts/build.py for the right target. WEVERYWHERE_VERSION was exported when
    this module loaded (resolve_version), so the child build bakes the identical
    version the testbed expects - no manual `scripts/build.py` step needed.
    """
    binary = host_weverywhere_binary(vm)
    if binary.exists():
        return binary

    plat = weverywhere_platform(vm)
    log(f"{binary.name} not found; building it via scripts/build.py {plat} ...")
    run(["uv", "run", str(REPO_ROOT / "scripts" / "build.py"), plat],
        cwd=REPO_ROOT)
    if not binary.exists():
        sys.exit(
            f"[testbed] scripts/build.py {plat} did not produce "
            f"{binary.relative_to(REPO_ROOT)}.\n"
            f"          Check the build output above."
        )
    log(f"built {binary.relative_to(REPO_ROOT)}")
    return binary


# --------------------------------------------------------------------------
# Deterministic identities (MAC, tap index)
# --------------------------------------------------------------------------

def vm_mac(vm: dict) -> str:
    """Stable locally-administered MAC derived from the hostname."""
    h = hashlib.sha256(vm["hostname"].encode()).digest()
    return "52:54:00:%02x:%02x:%02x" % (h[0], h[1], h[2])


def tap_name(vm: dict) -> str:
    idx = [v["hostname"] for v in all_vms()].index(vm["hostname"])
    return f"{vm_config.TAP_PREFIX}{idx}"


# --------------------------------------------------------------------------
# Networking address helpers
# --------------------------------------------------------------------------

def static_ip(vm: dict) -> ipaddress.IPv4Interface:
    return ipaddress.ip_interface(vm["static-ip"])


def host_bridge_iface() -> ipaddress.IPv4Interface:
    return ipaddress.ip_interface(vm_config.HOST_BRIDGE_CIDR)


def host_gateway_ip() -> str:
    """The address VMs use as their default gateway (the host's bridge IP)."""
    return str(host_bridge_iface().ip)


# --------------------------------------------------------------------------
# SSH keys
# --------------------------------------------------------------------------

def ensure_ssh_key(vm: dict) -> tuple[Path, str]:
    """Generate (once) and return this VM's private key path and public key."""
    key = vm_dir(vm) / "id_ed25519"
    if not key.exists():
        log(f"generating SSH key for {vm['hostname']} -> {key.relative_to(REPO_ROOT)}")
        run(["ssh-keygen", "-t", "ed25519", "-N", "", "-C",
             f"weverywhere-testbed-{vm['hostname']}", "-f", str(key)])
    pub = (key.with_suffix(".pub")).read_text().strip()
    return key, pub


def ssh_base_cmd(vm: dict) -> list[str]:
    key, _ = ensure_ssh_key(vm)
    return [
        "ssh", "-i", str(key),
        "-o", "StrictHostKeyChecking=no",
        "-o", "UserKnownHostsFile=/dev/null",
        "-o", "ConnectTimeout=5",
        "-o", "LogLevel=ERROR",
        f"{vm['username']}@{static_ip(vm).ip}",
    ]


def wait_for_ssh(vm: dict, timeout: int = 600) -> bool:
    """Poll TCP/22 then a real SSH command until the guest answers."""
    ip = str(static_ip(vm).ip)
    deadline = time.time() + timeout
    log(f"waiting for SSH on {ip} (up to {timeout}s) ...")
    while time.time() < deadline:
        with contextlib.suppress(OSError):
            with socket.create_connection((ip, 22), timeout=3):
                probe = run([*ssh_base_cmd(vm), "true"], check=False, capture=True)
                if probe.returncode == 0:
                    log(f"SSH is up on {ip}")
                    return True
        time.sleep(3)
    return False


# --------------------------------------------------------------------------
# Downloads (resumable, with progress)
# --------------------------------------------------------------------------

def download(url: str, dest: Path) -> Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists() and dest.stat().st_size > 0:
        log(f"using cached {dest.name} ({human_bytes(dest.stat().st_size)})")
        return dest
    tmp = dest.with_suffix(dest.suffix + ".partial")
    log(f"downloading {url}\n            -> {dest.relative_to(REPO_ROOT)}")
    # curl handles redirects, resume (-C -), and a live progress bar.
    run(["curl", "-fL", "--retry", "3", "-C", "-", "-o", str(tmp), url])
    tmp.rename(dest)
    return dest


# --------------------------------------------------------------------------
# ISO building (pure-python via pycdlib; no xorriso/genisoimage needed)
# --------------------------------------------------------------------------

def _iso_add(iso, data: bytes, name: str) -> None:
    """Add a file to a pycdlib ISO with a valid 8.3 name + real Joliet/RR name."""
    iso9660 = "/" + name.upper().replace("-", "_").replace(".", "_")[:8] + ".;1"
    iso.add_fp(BytesIO(data), len(data), iso9660,
               rr_name=name, joliet_path="/" + name)


def build_cloud_init_seed(vm: dict, dest: Path) -> Path:
    """
    Build a NoCloud seed ISO (label CIDATA) carrying cloud-init user-data,
    meta-data, network-config, plus the weverywhere binary itself. cloud-init
    provisions the user, SSH key, static IP and the boot service on first boot.
    """
    import pycdlib

    _, pubkey = ensure_ssh_key(vm)
    binary = host_weverywhere_binary(vm)
    guest_path = bootup_binary_path(vm)
    guest_dir = guest_path.rsplit("/", 1)[0]
    passwd_hash = _sha512_crypt(vm_config.LOGIN_PASSWORD)
    iface = static_ip(vm)

    meta_data = (
        # instance-id is tied to the build hash so a rebuild re-runs cloud-init.
        f"instance-id: {vm['hostname']}-{build_hash(vm)[:12]}\n"
        f"local-hostname: {vm['hostname']}\n"
    )

    user_data = f"""#cloud-config
hostname: {vm['hostname']}
fqdn: {vm['hostname']}
preserve_hostname: false
ssh_pwauth: true
users:
  - name: {vm['username']}
    groups: [wheel, sudo, adm]
    sudo: 'ALL=(ALL) NOPASSWD:ALL'
    shell: /bin/bash
    lock_passwd: false
    passwd: '{passwd_hash}'
    ssh_authorized_keys:
      - {pubkey}
chpasswd:
  expire: false
write_files:
  - path: /etc/systemd/system/weverywhere-test.service
    permissions: '0644'
    content: |
      [Unit]
      Description=weverywhere testbed boot binary
      After=network-online.target
      Wants=network-online.target
      [Service]
      Type=simple
      ExecStart={guest_path}
      Restart=on-failure
      [Install]
      WantedBy=multi-user.target
runcmd:
  - [ mkdir, -p, {guest_dir} ]
  - [ sh, -c, 'for d in /dev/disk/by-label/CIDATA /dev/disk/by-label/cidata /dev/sr0; do mount -o ro "$d" /mnt 2>/dev/null && break; done' ]
  - [ sh, -c, 'cp /mnt/wevebinary {guest_path}' ]
  - [ chmod, '0755', {guest_path} ]
  - [ umount, /mnt ]
  - [ systemctl, daemon-reload ]
  - [ systemctl, enable, weverywhere-test.service ]
power_state:
  mode: poweroff
  message: cloud-init provisioning complete
  timeout: 120
  condition: true
"""

    network_config = f"""version: 2
ethernets:
  netif:
    match:
      macaddress: '{vm_mac(vm)}'
    set-name: netif
    dhcp4: false
    addresses: [ {iface.with_prefixlen} ]
    routes:
      - to: 0.0.0.0/0
        via: {host_gateway_ip()}
    nameservers:
      addresses: [1.1.1.1, 9.9.9.9]
"""

    iso = pycdlib.PyCdlib()
    iso.new(interchange_level=3, joliet=3, rock_ridge="1.09", vol_ident="CIDATA")
    _iso_add(iso, user_data.encode(), "user-data")
    _iso_add(iso, meta_data.encode(), "meta-data")
    _iso_add(iso, network_config.encode(), "network-config")
    _iso_add(iso, binary.read_bytes(), "wevebinary")
    iso.write(str(dest))
    iso.close()
    log(f"built cloud-init seed {dest.relative_to(REPO_ROOT)}")
    return dest


def _sha512_crypt(password: str) -> str:
    """A $6$ (sha512) crypt hash for the guest login password, no deps needed."""
    try:
        import crypt  # removed in Python 3.13
        return crypt.crypt(password, crypt.mksalt(crypt.METHOD_SHA512))
    except (ImportError, AttributeError):
        salt = hashlib.sha256(os.urandom(16)).hexdigest()[:16]
        out = run(["openssl", "passwd", "-6", "-salt", salt, password],
                  capture=True)
        return out.stdout.strip()


def build_windows_config(vm: dict, dest: Path) -> Path:
    """
    Build the Windows config ISO: autounattend.xml (auto-detected by Windows
    Setup on removable media) plus a first-logon provisioning script, the
    weverywhere .exe, and the bundled OpenSSH server.
    """
    import pycdlib

    _, pubkey = ensure_ssh_key(vm)
    binary = host_weverywhere_binary(vm)
    openssh_zip = download(vm_config.WIN_OPENSSH_URL,
                           CACHE_DIR / "OpenSSH-Win64.zip")
    iface = static_ip(vm)

    autounattend = _windows_autounattend_xml(vm)
    # Batch files get CRLF line endings so cmd.exe parses them reliably.
    provision = _windows_provision_cmd(vm, iface, pubkey).replace("\n", "\r\n")
    stage = _windows_stage_cmd().replace("\n", "\r\n")

    iso = pycdlib.PyCdlib()
    iso.new(interchange_level=3, joliet=3, rock_ridge="1.09", vol_ident="WEVECFG")
    _iso_add(iso, autounattend.encode("utf-8"), "autounattend.xml")
    _iso_add(iso, stage.encode("utf-8"), "stage.cmd")
    _iso_add(iso, provision.encode("utf-8"), "provision.cmd")
    _iso_add(iso, binary.read_bytes(), "wevebinary.exe")
    _iso_add(iso, openssh_zip.read_bytes(), "OpenSSH-Win64.zip")
    iso.write(str(dest))
    iso.close()
    log(f"built Windows config ISO {dest.relative_to(REPO_ROOT)}")
    return dest


def _windows_autounattend_xml(vm: dict) -> str:
    """
    A best-effort unattended-install answer file for Windows 10/11 x64 (BIOS/MBR,
    single partition). Creates the local admin user, and in the specialize pass
    stages provision.cmd as SetupComplete.cmd (via stage.cmd on the config CD).
    Windows runs SetupComplete.cmd as SYSTEM at the end of install to set up
    OpenSSH + the static IP + the boot task and then power off - which is what
    lets the build detect completion. (We deliberately do NOT use
    FirstLogonCommands: SkipUserOOBE silently suppresses it, which is why an
    earlier version reached the desktop without ever provisioning or shutting
    down.)

    NOTE: the <InstallFrom> image index may need tweaking for your specific ISO
    edition (run `dism /Get-WimInfo` on the ISO's sources/install.wim). This is
    the piece most likely to need adjustment per Windows image.
    """
    user = vm["username"]
    pw = vm_config.LOGIN_PASSWORD
    host = vm["hostname"]
    return f"""<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="amd64"
      publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <SetupUILanguage><UILanguage>en-US</UILanguage></SetupUILanguage>
      <InputLocale>en-US</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64"
      publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <RunSynchronous>
        <!-- Let Windows 11 install on this BIOS VM with no TPM / Secure Boot.
             These LabConfig/MoSetup keys make Setup skip the hardware checks
             that would otherwise pop a "This PC can't run Windows 11" dialog
             and hang a headless install. Harmless on Windows 10. -->
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>1</Order><Path>reg add HKLM\\System\\Setup\\LabConfig /v BypassTPMCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>2</Order><Path>reg add HKLM\\System\\Setup\\LabConfig /v BypassSecureBootCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>3</Order><Path>reg add HKLM\\System\\Setup\\LabConfig /v BypassRAMCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>4</Order><Path>reg add HKLM\\System\\Setup\\LabConfig /v BypassCPUCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>5</Order><Path>reg add HKLM\\System\\Setup\\LabConfig /v BypassStorageCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>6</Order><Path>reg add HKLM\\System\\Setup\\MoSetup /v AllowUpgradesWithUnsupportedTPMOrCPU /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
      </RunSynchronous>
      <DiskConfiguration>
        <Disk wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID>
              <Active>true</Active><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallFrom><MetaData wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
            <Key>/IMAGE/INDEX</Key><Value>1</Value></MetaData></InstallFrom>
          <InstallTo><DiskID>0</DiskID><PartitionID>1</PartitionID></InstallTo>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <FullName>{user}</FullName>
        <Organization>weverywhere-testbed</Organization>
        <ProductKey><Key></Key></ProductKey>
      </UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Deployment" processorArchitecture="amd64"
      publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <RunSynchronous>
        <!-- Copy the config CD to C:\\weverywhere-setup and install our
             SetupComplete.cmd. Windows Setup runs SetupComplete.cmd (as SYSTEM)
             at the very end of installation REGARDLESS of OOBE/skip settings -
             unlike FirstLogonCommands, which SkipUserOOBE silently suppresses.
             That is what actually provisions SSH + the boot task and shuts the
             VM down, so the build can detect completion. stage.cmd only stages
             files here; the provisioning itself happens in SetupComplete.cmd. -->
        <RunSynchronousCommand wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
          <Order>1</Order>
          <Path>cmd /c for %d in (D E F G H I J K L) do @if exist %d:\\stage.cmd call %d:\\stage.cmd</Path>
        </RunSynchronousCommand>
      </RunSynchronous>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64"
      publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>{host}</ComputerName>
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <ProtectYourPC>3</ProtectYourPC>
        <SkipMachineOOBE>true</SkipMachineOOBE>
        <SkipUserOOBE>true</SkipUserOOBE>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
            <Name>{user}</Name><Group>Administrators</Group><DisplayName>{user}</DisplayName>
            <Password><Value>{pw}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
    </component>
  </settings>
</unattend>
"""


def _windows_stage_cmd() -> str:
    """
    Runs in the specialize pass off the config CD (%~d0 = that CD). Copies the
    CD to C:\\weverywhere-setup and installs provision.cmd as the SetupComplete
    hook Windows runs at the end of install. Kept tiny and CD-relative; the real
    work is in SetupComplete.cmd, which runs from the C: copies.
    """
    return (
        "@echo off\n"
        "set CD=%~d0\n"
        "md C:\\weverywhere-setup 2>nul\n"
        "xcopy /y /h /e /i \"%CD%\\*.*\" C:\\weverywhere-setup\\\n"
        "md C:\\Windows\\Setup\\Scripts 2>nul\n"
        "copy /y \"%CD%\\provision.cmd\" C:\\Windows\\Setup\\Scripts\\SetupComplete.cmd\n"
    )


def _windows_provision_cmd(vm, iface, pubkey: str) -> str:
    """
    Installed as C:\\Windows\\Setup\\Scripts\\SetupComplete.cmd (see stage.cmd),
    which Windows Setup runs as SYSTEM at the very end of installation. Installs
    OpenSSH from the bundled zip, drops our public key, sets the hard-coded
    static IP, registers the weverywhere boot task, then shuts the VM down so the
    build detects completion. Idempotent and logged to
    C:\\weverywhere-setup\\provision.log for post-mortems.
    """
    guest_path = bootup_binary_path(vm)
    guest_dir = guest_path.rsplit("\\", 1)[0]
    ip = str(iface.ip)
    mask = str(iface.network.netmask)
    gw = host_gateway_ip()
    return f"""@echo off
set SRC=C:\\weverywhere-setup
set LOG=%SRC%\\provision.log
echo [provision] start %DATE% %TIME%>>"%LOG%" 2>&1

rem --- weverywhere binary into place ---
md "{guest_dir}" 2>nul
copy /y "%SRC%\\wevebinary.exe" "{guest_path}">>"%LOG%" 2>&1

rem --- OpenSSH server from the bundled zip (idempotent) ---
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference='Continue';" ^
  "if (-not (Test-Path 'C:\\Program Files\\OpenSSH-Win64\\sshd.exe')) {{ Expand-Archive -Force '%SRC%\\OpenSSH-Win64.zip' 'C:\\Program Files' }};" ^
  "if (-not (Get-Service sshd -ErrorAction SilentlyContinue)) {{ & 'C:\\Program Files\\OpenSSH-Win64\\install-sshd.ps1' }};" ^
  "Set-Service sshd -StartupType Automatic;" ^
  "Start-Service sshd;" ^
  "if (-not (Get-NetFirewallRule -Name sshd -ErrorAction SilentlyContinue)) {{ New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH Server' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 }}">>"%LOG%" 2>&1

rem --- authorized_keys for the admin account (strict ACL required by sshd) ---
md "C:\\ProgramData\\ssh" 2>nul
echo {pubkey}>"C:\\ProgramData\\ssh\\administrators_authorized_keys"
icacls "C:\\ProgramData\\ssh\\administrators_authorized_keys" /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F">>"%LOG%" 2>&1

rem --- hard-coded static IP on the first physical adapter ---
for /f "tokens=*" %%N in ('powershell -NoProfile -Command "(Get-NetAdapter -Physical | Sort-Object ifIndex | Select-Object -First 1).Name"') do set NIC=%%N
netsh interface ip set address name="%NIC%" static {ip} {mask} {gw}>>"%LOG%" 2>&1

rem --- run the weverywhere binary at every boot ---
schtasks /create /tn weverywhere-test /tr "\"{guest_path}\"" /sc onstart /ru SYSTEM /rl HIGHEST /f>>"%LOG%" 2>&1

echo [provision] done %DATE% %TIME%>>"%LOG%" 2>&1
rem --- power off so create-test-vm-images.py sees the install finish ---
shutdown /s /t 10 /c "weverywhere provisioning complete"
"""


# --------------------------------------------------------------------------
# Build manifest (rebuild-on-change)
# --------------------------------------------------------------------------

def build_hash(vm: dict) -> str:
    """
    Hash of everything a built image depends on. Changing the VM entry, the
    globals it uses, the base image, or the weverywhere binary changes this and
    forces a rebuild.
    """
    binary = host_weverywhere_binary(vm)
    binary_id = ""
    if binary.exists():
        st = binary.stat()
        binary_id = f"{binary.name}:{st.st_size}:{int(st.st_mtime)}"

    material = {
        "vm": vm,
        "version": WEVERYWHERE_VERSION,
        "disk_gb": vm_config.DISK_SIZE_GB,
        "login_password": vm_config.LOGIN_PASSWORD,
        "host_bridge": vm_config.HOST_BRIDGE_CIDR,
        "image_source": vm_config.IMAGE_SOURCES.get(vm["os"].lower(), "windows-iso"),
        "binary": binary_id,
    }
    blob = json.dumps(material, sort_keys=True).encode()
    return hashlib.sha256(blob).hexdigest()


def manifest_path(vm: dict) -> Path:
    return vm_dir(vm) / "build-manifest.json"


def disk_path(vm: dict) -> Path:
    return vm_dir(vm) / "disk.qcow2"


def needs_rebuild(vm: dict) -> bool:
    if not disk_path(vm).exists():
        return True
    mp = manifest_path(vm)
    if not mp.exists():
        return True
    try:
        saved = json.loads(mp.read_text()).get("build_hash")
    except (json.JSONDecodeError, OSError):
        return True
    return saved != build_hash(vm)


def write_manifest(vm: dict) -> None:
    manifest_path(vm).write_text(json.dumps({
        "hostname": vm["hostname"],
        "os": vm["os"],
        "version": WEVERYWHERE_VERSION,
        "build_hash": build_hash(vm),
        "built_at": time.strftime("%Y-%m-%d %H:%M:%S UTC", time.gmtime()),
    }, indent=2) + "\n")


# --------------------------------------------------------------------------
# QEMU
# --------------------------------------------------------------------------

def kvm_available() -> bool:
    return os.path.exists("/dev/kvm") and os.access("/dev/kvm", os.R_OK | os.W_OK)


def _accel_args() -> list[str]:
    if kvm_available():
        return ["-enable-kvm", "-cpu", "host"]
    log("WARNING: /dev/kvm not available - falling back to slow TCG emulation. "
        "A Windows install under TCG is impractically slow; run this on a host "
        "with KVM.")
    return ["-accel", "tcg", "-cpu", "max"]


def display_args(*, gui: bool = False) -> list[str]:
    """
    qemu display args. Default is fully headless; the diagnostic --debug flag
    opens a normal qemu GTK window so you can watch a boot/install and interact
    with it (e.g. answer a "Press any key to boot from CD" prompt). The GTK
    window needs a local X/Wayland session on the host.
    """
    return ["-display", "gtk"] if gui else ["-display", "none"]


def qemu_base(vm: dict, *, disp: list[str]) -> list[str]:
    return [
        "qemu-system-x86_64",
        *_accel_args(),
        "-machine", "pc",  # i440fx: IDE + e1000 work with stock Windows drivers
        "-smp", str(vm["cpu"]),
        "-m", str(vm["ram-mb"]),
        "-rtc", "base=utc",
        "-serial", f"file:{vm_dir(vm) / 'serial.log'}",
        "-monitor", "none",
        *disp,
    ]


def _disk_args(vm: dict) -> list[str]:
    disk = disk_path(vm)
    if is_windows(vm):
        # IDE: built into every Windows image, no virtio driver injection needed.
        return ["-drive", f"file={disk},format=qcow2,if=ide"]
    # Linux cloud images ship virtio drivers.
    return ["-drive", f"file={disk},format=qcow2,if=virtio"]


def _nic_args(vm: dict, netdev: str) -> list[str]:
    model = "e1000" if is_windows(vm) else "virtio-net-pci"
    return ["-device", f"{model},netdev=n0,mac={vm_mac(vm)}",
            "-netdev", netdev]


def qemu_install_cmd(vm: dict, *, disp: list[str]) -> list[str]:
    """qemu command for the unattended install/provision boot (user-mode net)."""
    cmd = qemu_base(vm, disp=disp)
    cmd += _disk_args(vm)
    # User-mode networking: gives the guest outbound access with zero host
    # interfaces to create or tear down.
    cmd += _nic_args(vm, "user,id=n0")
    if is_windows(vm):
        win_iso = CACHE_DIR / vm_config.WINDOWS_ISO_NAME
        cfg_iso = vm_dir(vm) / "config.iso"
        cmd += ["-drive", f"file={win_iso},media=cdrom,index=2,readonly=on"]
        cmd += ["-drive", f"file={cfg_iso},media=cdrom,index=3,readonly=on"]
        cmd += ["-boot", "once=d"]  # boot the install CD first time only
    else:
        seed = vm_dir(vm) / "seed.iso"
        cmd += ["-drive", f"file={seed},media=cdrom,index=2,readonly=on"]
    return cmd


def qemu_run_cmd(vm: dict, tap: str, qmp_sock: Path, *, disp: list[str]) -> list[str]:
    """qemu command for a normal boot on the host bridge via a tap device."""
    cmd = qemu_base(vm, disp=disp)
    cmd += _disk_args(vm)
    cmd += _nic_args(vm, f"tap,id=n0,ifname={tap},script=no,downscript=no")
    # QMP socket lets the run script request a clean ACPI shutdown on exit.
    cmd += ["-qmp", f"unix:{qmp_sock},server,nowait"]
    return cmd


def qmp_powerdown(qmp_sock: Path) -> bool:
    """Ask a running qemu (via its QMP socket) to perform an ACPI shutdown."""
    try:
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(5)
            s.connect(str(qmp_sock))
            s.recv(4096)  # greeting
            s.sendall(b'{"execute":"qmp_capabilities"}\n')
            s.recv(4096)
            s.sendall(b'{"execute":"system_powerdown"}\n')
            s.recv(4096)
        return True
    except OSError:
        return False


# --------------------------------------------------------------------------
# Host bridge + tap networking (run phase only) with guaranteed teardown
# --------------------------------------------------------------------------

class HostNetwork:
    """
    Creates a bridge holding the host's gateway IP, plus one tap per VM, and
    removes every interface it created on exit. Register once as a context
    manager; teardown also runs on atexit and on SIGINT/SIGTERM/SIGHUP so no
    virtual interfaces are ever left behind.
    """

    def __init__(self) -> None:
        self.bridge = vm_config.BRIDGE_NAME
        self._created: list[tuple[str, str]] = []  # (kind, name), teardown LIFO
        self._torn_down = False

    # -- lifecycle --------------------------------------------------------
    def __enter__(self) -> "HostNetwork":
        atexit.register(self.teardown)
        for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
            with contextlib.suppress(ValueError):
                signal.signal(sig, self._on_signal)
        self.setup()
        return self

    def __exit__(self, *exc) -> None:
        self.teardown()

    def _on_signal(self, signum, frame) -> None:
        log(f"received signal {signum}; tearing down network ...")
        self.teardown()
        sys.exit(128 + signum)

    # -- setup ------------------------------------------------------------
    def setup(self) -> None:
        host = host_bridge_iface()
        log(f"creating bridge {self.bridge} ({host.with_prefixlen})")
        run(["ip", "link", "add", self.bridge, "type", "bridge"], sudo=True)
        self._created.append(("link", self.bridge))
        run(["ip", "addr", "add", str(host.with_prefixlen), "dev", self.bridge],
            sudo=True, check=False)
        run(["ip", "link", "set", self.bridge, "up"], sudo=True)

        for vm in all_vms():
            tap = tap_name(vm)
            log(f"creating tap {tap} for {vm['hostname']} ({static_ip(vm).ip})")
            run(["ip", "tuntap", "add", "dev", tap, "mode", "tap",
                 "user", str(os.getenv("USER", "root"))], sudo=True)
            self._created.append(("link", tap))
            run(["ip", "link", "set", tap, "master", self.bridge], sudo=True)
            run(["ip", "link", "set", tap, "up"], sudo=True)

    # -- teardown ---------------------------------------------------------
    def teardown(self) -> None:
        if self._torn_down:
            return
        self._torn_down = True
        log("tearing down host network interfaces ...")
        for kind, name in reversed(self._created):
            run(["ip", "link", "del", name], sudo=True, check=False)
        self._created.clear()
