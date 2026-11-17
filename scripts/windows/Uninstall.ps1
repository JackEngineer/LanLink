$ErrorActionPreference = "Stop"
$install = Join-Path $env:LOCALAPPDATA "Programs/LANLink"
$running = Get-Process -Name lanlink-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $install "lanlink-agent.exe") }
if ($running) {
  $running | Stop-Process
  $running | Wait-Process -Timeout 10
}
foreach ($folder in @("Programs", "Startup")) {
  $link = Join-Path ([Environment]::GetFolderPath($folder)) "LANLink.lnk"
  if (Test-Path $link) { Remove-Item $link }
}
if (Test-Path $install) { Remove-Item $install -Recurse }
Write-Host "LANLink removed. Identity, history and received files have been retained."
