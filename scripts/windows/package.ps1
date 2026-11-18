param([ValidateSet("x86_64-pc-windows-msvc")][string]$Target = "x86_64-pc-windows-msvc")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "../..")
$Version = (Get-Content "package.json" -Raw | ConvertFrom-Json).version
npm run build
if ($LASTEXITCODE -ne 0) { throw "Frontend build failed" }
$resourceCompiler = Get-Command rc.exe -ErrorAction SilentlyContinue
if ($resourceCompiler) {
  $rc = $resourceCompiler.Source
} else {
  $sdk = Join-Path ${env:ProgramFiles(x86)} "Windows Kits/10/bin"
  $rc = Get-ChildItem "$sdk/*/x64/rc.exe" -ErrorAction SilentlyContinue |
    Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
    Select-Object -First 1 -ExpandProperty FullName
}
if (-not $rc) { throw "Windows SDK rc.exe is required to embed the application icon" }
$resourceDir = New-Item -ItemType Directory -Force "target/lanlink-resources"
$iconHash = (Get-FileHash "assets/branding/lanlink.ico" -Algorithm SHA256).Hash
$resourceHash = (Get-FileHash "scripts/windows/lanlink.rc" -Algorithm SHA256).Hash
$resource = Join-Path $resourceDir.FullName "lanlink-$iconHash-$resourceHash.res"
& $rc /nologo /I (Join-Path $PWD "assets/branding") /fo $resource "scripts/windows/lanlink.rc"
if ($LASTEXITCODE -ne 0) { throw "Icon resource compilation failed" }
cargo rustc --release --locked --target-dir target --target $Target -p lanlink-agent --bin lanlink-agent -- -C "link-arg=$resource"
if ($LASTEXITCODE -ne 0) { throw "Agent build failed" }
$out = "dist/windows-$Target"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force "$out/web" | Out-Null
Copy-Item "target/$Target/release/lanlink-agent.exe" "$out/lanlink-agent.exe"
Copy-Item "assets/branding/lanlink.ico" "$out/lanlink.ico"
Copy-Item "apps/web/dist/*" "$out/web" -Recurse -Force
Copy-Item "scripts/windows/Install.ps1", "scripts/windows/Launch.ps1", "scripts/windows/Uninstall.ps1" $out
Copy-Item LICENSE "$out/LICENSE"
Compress-Archive -Path "$out/*" -DestinationPath "dist/LANLink-$Version-$Target.zip" -Force
Get-FileHash "dist/LANLink-$Version-$Target.zip" -Algorithm SHA256
