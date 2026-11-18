param([switch]$StartAtLogin, [switch]$DisableStartAtLogin)
$ErrorActionPreference = "Stop"
if ($StartAtLogin -and $DisableStartAtLogin) { throw "Choose either enable or disable login startup" }
$destination = Join-Path $env:LOCALAPPDATA "Programs/LANLink"
$current = Join-Path $destination "current"
$data = Join-Path $env:LOCALAPPDATA "LANLink/LANLink/data"
New-Item -ItemType Directory -Path $destination -Force | Out-Null
New-Item -ItemType Directory -Path $data -Force | Out-Null
$lock = [IO.File]::Open((Join-Path $data "update.lock"), "OpenOrCreate", "ReadWrite", "ReadWrite")
try {
  $lock.Lock(0, 1)
  $running = Get-Process -Name lanlink-agent -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -in @((Join-Path $destination "lanlink-agent.exe"), (Join-Path $current "lanlink-agent.exe"))
  }
  if ($running) {
    $running | Stop-Process
    $running | Wait-Process -Timeout 10
  }
  New-Item -ItemType Directory -Path $current -Force | Out-Null
  Copy-Item "$PSScriptRoot/*" $current -Recurse -Force
  Copy-Item "$PSScriptRoot/Launch.ps1", "$PSScriptRoot/Uninstall.ps1", "$PSScriptRoot/lanlink.ico" $destination -Force
  $previous = Join-Path $destination ".previous"
  if (Test-Path $previous -PathType Container) {
    $retired = Join-Path $destination (".retired-" + [Guid]::NewGuid().ToString("N"))
    [IO.Directory]::Move($previous, $retired)
    Remove-Item -LiteralPath $retired -Recurse -Force -ErrorAction SilentlyContinue
  }
} finally {
  $lock.Dispose()
}
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
