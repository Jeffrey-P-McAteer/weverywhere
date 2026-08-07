
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
    },
]

