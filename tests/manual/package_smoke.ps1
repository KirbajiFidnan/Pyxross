$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$DistDir = Join-Path $RepoRoot 'dist'
$Archive = @(Get-ChildItem $DistDir -Filter 'pyxross-*-windows-x86_64-portable.zip' -File)
if ($Archive.Count -ne 1) { throw 'BLOCKED: expected exactly one Windows portable ZIP in dist' }
$Hash = (Get-FileHash $Archive.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
$Expected = (Get-Content (Join-Path $DistDir 'SHA256SUMS-windows')).Split()[0]
if ($Hash -ne $Expected) { throw 'FAIL: checksum mismatch' }
$Listing = [System.IO.Compression.ZipFile]::OpenRead($Archive.FullName).Entries.FullName
foreach ($Suffix in @('/pyxross.exe','/themes/builtin/theme.json','/themes/builtin/atlas.png','/README.md','/LICENSE')) {
    if (-not ($Listing | Where-Object { $_ -like "*$Suffix" })) { throw "FAIL: ZIP missing $Suffix" }
}
Write-Output 'PASS: Windows ZIP contents and checksum'
Write-Output 'PASS: portable package does not include PDB by default'
Write-Output 'BLOCKED: native Windows/MSVC launch smoke must run on Windows'
