param([switch]$StartAtLogin, [switch]$DisableStartAtLogin)
$ErrorActionPreference = "Stop"
if ($StartAtLogin -and $DisableStartAtLogin) { throw "Choose either enable or disable login startup" }
$destination = Join-Path $env:LOCALAPPDATA "Programs/LANLink"
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$running = Get-Process -Name lanlink-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $destination "lanlink-agent.exe") }
if ($running) {
  $running | Stop-Process
  $running | Wait-Process -Timeout 10
}
Copy-Item "$PSScriptRoot/*" $destination -Recurse -Force
$shell = New-Object -ComObject WScript.Shell
$menu = [Environment]::GetFolderPath("Programs")
$shortcut = $shell.CreateShortcut((Join-Path $menu "LANLink.lnk"))
$shortcut.TargetPath = "$env:SystemRoot/System32/WindowsPowerShell/v1.0/powershell.exe"
$shortcut.Arguments = "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$destination/Launch.ps1`""
$shortcut.WorkingDirectory = $destination
$shortcut.IconLocation = "$destination/lanlink.ico,0"
$shortcut.Save()
$startupLink = Join-Path ([Environment]::GetFolderPath("Startup")) "LANLink.lnk"
if (-not $DisableStartAtLogin -and ($StartAtLogin -or (Test-Path $startupLink))) {
  $startup = $shell.CreateShortcut($startupLink)
  $startup.TargetPath = $shortcut.TargetPath
  $startup.Arguments = "$($shortcut.Arguments) -BackgroundOnly"
  $startup.WorkingDirectory = $destination
  $startup.IconLocation = $shortcut.IconLocation
  $startup.Save()
}
if ($DisableStartAtLogin) {
  if (Test-Path $startupLink) { Remove-Item $startupLink }
}
Write-Host "LANLink installed. Allow private-network access when Windows Firewall prompts. Do not allow public networks."
& "$destination/Launch.ps1"
