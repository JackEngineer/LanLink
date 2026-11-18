param([ValidateSet("x86_64-pc-windows-msvc")][string]$Target = "x86_64-pc-windows-msvc")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "../..")
npm run build
if ($LASTEXITCODE -ne 0) { throw "Frontend build failed" }
cargo build --release --locked --target-dir target --target $Target -p lanlink-agent
if ($LASTEXITCODE -ne 0) { throw "Agent build failed" }
$out = "dist/windows-$Target"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force "$out/web" | Out-Null
Copy-Item "target/$Target/release/lanlink-agent.exe" "$out/lanlink-agent.exe"
Copy-Item "apps/web/dist/*" "$out/web" -Recurse -Force
Copy-Item "scripts/windows/Install.ps1", "scripts/windows/Launch.ps1", "scripts/windows/Uninstall.ps1" $out
Copy-Item LICENSE "$out/LICENSE"
Compress-Archive -Path "$out/*" -DestinationPath "dist/LANLink-0.2.0-$Target.zip" -Force
Get-FileHash "dist/LANLink-0.2.0-$Target.zip" -Algorithm SHA256
