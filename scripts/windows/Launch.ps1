param([switch]$BackgroundOnly)
$ErrorActionPreference = "Stop"
$binary = Join-Path $PSScriptRoot "lanlink-agent.exe"
$logs = Join-Path $env:LOCALAPPDATA "LANLink/LANLink/data/logs"
New-Item -ItemType Directory -Path $logs -Force | Out-Null
$existing = Get-Process -Name lanlink-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $binary }
if (-not $existing) {
  Start-Process -FilePath $binary -ArgumentList @("--web-dir", "`"$PSScriptRoot/web`"") -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs "launcher.stdout.log") -RedirectStandardError (Join-Path $logs "launcher.stderr.log")
}
if (-not $BackgroundOnly) {
  for ($attempt = 0; $attempt -lt 40; $attempt++) {
    try {
      $health = Invoke-RestMethod "http://127.0.0.1:38761/api/v1/health" -TimeoutSec 1
      if ($health.status -eq "ok" -and $health.service -eq "lanlink") { Start-Process "http://127.0.0.1:38761"; exit 0 }
    } catch { Start-Sleep -Milliseconds 250 }
  }
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.MessageBox]::Show("LANLink failed to start. Check the logs in: $logs", "LANLink") | Out-Null
  throw "LANLink did not start. Check $logs"
}
