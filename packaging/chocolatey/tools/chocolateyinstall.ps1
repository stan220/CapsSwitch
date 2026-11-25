$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$packageArgs = @{
  packageName   = 'capsswitch'
  url           = 'https://github.com/stan220/CapsSwitch/releases/download/v__VERSION__/capsswitch-windows-i686.zip'
  url64bit      = 'https://github.com/stan220/CapsSwitch/releases/download/v__VERSION__/capsswitch-windows-x86_64.zip'
  checksum      = '__CHECKSUM32__'
  checksum64    = '__CHECKSUM64__'
  checksumType  = 'sha256'
  checksumType64 = 'sha256'
  unzipLocation = $toolsDir
}

Install-ChocolateyZipPackage @packageArgs
