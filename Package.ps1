param([string]$Output = "Distribution\Flux", [switch]$Portable, [switch]$Install, [string]$InstallDirectory = "$env:LOCALAPPDATA\Programs\Flux")
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
if (Test-Path $Output) { Remove-Item $Output -Recurse -Force }
New-Item -ItemType Directory -Force -Path $Output | Out-Null
Copy-Item "target\release\Flux.exe" $Output
Copy-Item "target\release\Flux.pdb" $Output
foreach ($Module in Get-ChildItem "Modules" -Directory) {
    $Manifest = Join-Path $Module.FullName "Module.ini"
    $Library = (Select-String -Path $Manifest -Pattern "^Library=(.+)$").Matches[0].Groups[1].Value.Trim()
    $Target = Join-Path $Output "Modules\$($Module.Name)"
    New-Item -ItemType Directory -Force -Path $Target | Out-Null
    Copy-Item $Manifest $Target
    Copy-Item (Join-Path "target\release" $Library) $Target
    $Pdb = Join-Path "target\release" ([IO.Path]::ChangeExtension($Library, ".pdb"))
    if (Test-Path $Pdb) { Copy-Item $Pdb $Target }
}
if ($Portable) { New-Item -ItemType File -Force -Path (Join-Path $Output "Flux.portable") | Out-Null }
Write-Output "Packaged into $Output"
if ($Install) {
    $Executable = Join-Path $InstallDirectory "Flux.exe"
    Get-Process -Name Flux -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $Executable } | ForEach-Object { $_.CloseMainWindow() | Out-Null; if (-not $_.WaitForExit(5000)) { $_.Kill() } }
    New-Item -ItemType Directory -Force -Path $InstallDirectory | Out-Null
    Copy-Item (Join-Path $Output "*") $InstallDirectory -Recurse -Force
    $Process = Start-Process -FilePath $Executable -ArgumentList "--register-shell" -Wait -PassThru
    if ($Process.ExitCode -ne 0) { throw "Shell registration failed" }
    Write-Output "Installed into $InstallDirectory and registered as the default file manager"
}
