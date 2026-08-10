
VMS = [
    {
      'os': 'windows',
      'arch': 'x86_64',
      'hostname': 'win-test01',
      'username': 'wintest01',
      'cpu': 2,
      'ram-mb': 8192,
      'static-ip': '10.0.0.1/16',
      'bootup-binary': r'C:\weverywhere\weverywhere-{VERSION}-windows-x64.exe',
      'os-packages': [
        'vim', 'cat'
      ],
      'initial-weverywhere.toml': '''
[identity]
name = "win-test01-from-config-file"

[limits.trusted]
max_cpu_instructions = 4611686018427387904 # 2**62
max_memory_bytes = 4611686018427387904

[limits.untrusted]
max_cpu_instructions = 65536
max_memory_bytes = 65536

[[peer]]
ipv4 = "10.0.0.2"

[[peer]]
ipv4 = "10.0.10.2"

''',
    },
    {
      'os': 'windows',
      'arch': 'x86_64',
      'hostname': 'win-test02',
      'username': 'wintest02',
      'cpu': 4,
      'ram-mb': 8192,
      'static-ip': '10.0.0.2/16',
      'bootup-binary': r'C:\weverywhere\weverywhere-{VERSION}-windows-x64.exe',
      'os-packages': [
        'vim', 'cat'
      ],
    },
    {
      'os': 'fedora',
      'arch': 'x86_64',
      'hostname': 'linux-test01',
      'username': 'linuxtest01',
      'cpu': 2,
      'ram-mb': 4096,
      'static-ip': '10.0.10.1/16',
      'bootup-binary': r'/opt/weverywhere/weverywhere-{VERSION}-linux-x64',
      'os-packages': [
        'vim', 'nmap'
      ],
    },
    {
      'os': 'ubuntu',
      'arch': 'x86_64',
      'hostname': 'linux-test02',
      'username': 'linuxtest02',
      'cpu': 2,
      'ram-mb': 4096,
      'static-ip': '10.0.10.2/16',
      'bootup-binary': r'/opt/weverywhere/weverywhere-{VERSION}-linux-x64',
      'os-packages': [
        'vim', 'nmap'
      ],
    },
]


# ---------------------------------------------------------------------------
# Global testbed settings. Changing anything a VM depends on (its entry above,
# the disk size, the host bridge, or the image it is built from) changes that
# VM's build manifest hash, so create-test-vm-images.py rebuilds it to match.
# ---------------------------------------------------------------------------

# The host's own address on the shared VM bridge. Every VM's static-ip above
# lives in this network and reaches the host (and is reached from the host for
# SSH) at this address. Must NOT collide with any VM static-ip.
HOST_BRIDGE_CIDR = '10.0.255.254/16'

# Linux interface names are capped at 15 chars; these stay well under.
BRIDGE_NAME = 'brweve'          # host bridge all VM taps attach to
TAP_PREFIX  = 'tapweve'         # per-VM tap devices: tapweve0, tapweve1, ...

# Size of each VM's system disk (thin-provisioned in the qcow2).
DISK_SIZE_GB = 56

# Console/login password for the created user. SSH itself uses the generated
# key pair (testbed/vms/<hostname>/id_ed25519); this is only for the local
# console / sudo, and to let you log in if you ever open the VM window.
LOGIN_PASSWORD = 'weverywhere'

# Base images the VMs are built from. Linux uses ready-made cloud qcow2 images
# provisioned by cloud-init; Windows is installed from an ISO you provide (see
# WINDOWS_ISO_NAME) because Microsoft does not publish a redistributable image.
IMAGE_SOURCES = {
    'fedora': 'https://download.fedoraproject.org/pub/fedora/linux/releases/43/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-43-1.6.x86_64.qcow2',
    'ubuntu': 'https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img',
}

# Windows installation ISO. Place the file at testbed/cache/<WINDOWS_ISO_NAME>.
# create-test-vm-images.py prints download instructions and waits if it is
# missing (Microsoft requires an interactive download / license acceptance).
WINDOWS_ISO_NAME = 'windows-x64.iso'

# Win32-OpenSSH release bundled onto the Windows config ISO so the guest gets an
# SSH server without needing internet during install. Downloaded to testbed/cache/.
WIN_OPENSSH_URL = 'https://github.com/PowerShell/Win32-OpenSSH/releases/download/v9.8.1.0p1-Preview/OpenSSH-Win64.zip'

