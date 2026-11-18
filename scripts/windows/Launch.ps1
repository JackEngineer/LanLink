param([switch]$BackgroundOnly)
$ErrorActionPreference = "Stop"
$current = Join-Path $PSScriptRoot "current"
$previous = Join-Path $PSScriptRoot ".previous"
if (-not (Test-Path $current) -and -not (Test-Path $previous)) {
  $current = $PSScriptRoot
}
$binary = Join-Path $current "lanlink-agent.exe"
$data = Join-Path $env:LOCALAPPDATA "LANLink/LANLink/data"
$logs = Join-Path $data "logs"
New-Item -ItemType Directory -Path $logs -Force | Out-Null
$existing = Get-Process -Name lanlink-agent -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $binary }
if (-not $existing -and (Test-Path $previous -PathType Container)) {
  $lock = [IO.File]::Open((Join-Path $data "update.lock"), "OpenOrCreate", "ReadWrite", "ReadWrite")
  try {
    try { $lock.Lock(0, 1) } catch [IO.IOException] { exit 0 }
    if ((Test-Path $previous -PathType Container) -and -not (Test-Path (Join-Path $previous ".committed"))) {
      if (Test-Path $current) {
        $rejected = Join-Path $PSScriptRoot (".rejected-" + [Guid]::NewGuid().ToString("N"))
        [IO.Directory]::Move($current, $rejected)
      }
      [IO.Directory]::Move($previous, $current)
      if ($rejected) { Remove-Item -LiteralPath $rejected -Recurse -Force -ErrorAction SilentlyContinue }
    }
  } finally {
    $lock.Dispose()
  }
}
if (-not $existing) {
  Start-Process -FilePath $binary -ArgumentList @("--web-dir", "`"$current/web`"") -WindowStyle Hidden -RedirectStandardOutput (Join-Path $logs "launcher.stdout.log") -RedirectStandardError (Join-Path $logs "launcher.stderr.log")
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
