$ErrorActionPreference = 'SilentlyContinue'
Get-Process -Name caps-switch | Stop-Process -Force
schtasks.exe /Delete /TN CapsSwitch /F
