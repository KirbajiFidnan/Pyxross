$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..')).Path
$DistDir = Join-Path $RepoRoot 'dist'
$Target = 'x86_64-pc-windows-msvc'

function Stop-Packaging([string]$Message) { throw "error: $Message" }
function Require-Command([string]$Name) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) { Stop-Packaging "required tool '$Name' is unavailable (environment blocker)" }
}

Require-Command 'cargo'
if (-not $env:SOURCE_DATE_EPOCH) { Stop-Packaging 'SOURCE_DATE_EPOCH is required for reproducible output (environment blocker)' }
if ($env:SOURCE_DATE_EPOCH -notmatch '^[0-9]+$') { Stop-Packaging 'SOURCE_DATE_EPOCH must be a non-negative integer' }
$Metadata = cargo metadata --locked --no-deps --format-version 1 --manifest-path (Join-Path $RepoRoot 'Cargo.toml') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { Stop-Packaging 'cargo metadata failed; refusing to stage a package (environment/build blocker)' }
if (-not $Metadata) { Stop-Packaging 'compatible Rust toolchain or locked metadata is unavailable (environment blocker)' }
$Package = $Metadata.packages[0].name
$Version = $Metadata.packages[0].version
if ($Package -ne 'pyxross') { Stop-Packaging "Cargo package is '$Package', expected pyxross" }
cargo build --locked --release --target $Target --manifest-path (Join-Path $RepoRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { Stop-Packaging "cargo build failed for $Target; refusing to stage a package (build blocker)" }
$Binary = Join-Path $RepoRoot "target\$Target\release\$Package.exe"
if (-not (Test-Path $Binary)) { Stop-Packaging "built executable not found at $Binary (script/build error)" }

$Theme = Join-Path $RepoRoot 'assets\themes\default\theme.json'
$Atlas = Join-Path $RepoRoot 'assets\themes\default\atlas.png'
foreach ($Path in @($Theme, $Atlas, (Join-Path $RepoRoot 'LICENSE'), (Join-Path $RepoRoot 'packaging\package-README.md'))) {
    if (-not (Test-Path $Path)) { Stop-Packaging "missing required resource $Path" }
}
$PackageRoot = Join-Path $DistDir "${Package}-${Version}-windows-x86_64-portable"
$Archive = Join-Path $DistDir "${Package}-${Version}-windows-x86_64-portable.zip"
Remove-Item $PackageRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $PackageRoot 'themes\builtin') | Out-Null
Copy-Item $Binary (Join-Path $PackageRoot "$Package.exe")
Copy-Item $Theme (Join-Path $PackageRoot 'themes\builtin\theme.json')
Copy-Item $Atlas (Join-Path $PackageRoot 'themes\builtin\atlas.png')
Copy-Item (Join-Path $RepoRoot 'packaging\package-README.md') (Join-Path $PackageRoot 'README.md')
Copy-Item (Join-Path $RepoRoot 'LICENSE') (Join-Path $PackageRoot 'LICENSE')
if (Test-Path $Archive) { Remove-Item $Archive -Force }
$Epoch = ([DateTimeOffset]'1970-01-01T00:00:00Z').AddSeconds([long]$env:SOURCE_DATE_EPOCH)
$ZipStream = [System.IO.File]::Open($Archive, [System.IO.FileMode]::CreateNew)
$Zip = New-Object -TypeName System.IO.Compression.ZipArchive -ArgumentList @($ZipStream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
try {
    $Files = Get-ChildItem $PackageRoot -File -Recurse | Sort-Object FullName
    foreach ($File in $Files) {
        $Relative = $File.FullName.Substring($PackageRoot.Length + 1).Replace('\', '/')
        $Entry = $Zip.CreateEntry($Relative, [System.IO.Compression.CompressionLevel]::Optimal)
        $Entry.LastWriteTime = $Epoch
        $Input = [System.IO.File]::OpenRead($File.FullName)
        $Output = $Entry.Open()
        try { $Input.CopyTo($Output) } finally { $Output.Dispose(); $Input.Dispose() }
    }
} finally { $Zip.Dispose(); $ZipStream.Dispose() }
$Hash = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content (Join-Path $DistDir 'SHA256SUMS-windows') "$hash  $(Split-Path $Archive -Leaf)" -Encoding ascii
if (Get-Command llvm-readobj -ErrorAction SilentlyContinue) { llvm-readobj --file-headers (Join-Path $PackageRoot "$Package.exe") | Out-Null }
elseif (Get-Command dumpbin -ErrorAction SilentlyContinue) { dumpbin /headers (Join-Path $PackageRoot "$Package.exe") | Out-Null }
Remove-Item $PackageRoot -Recurse -Force
Write-Output "created: $Archive"
Write-Output "checksums: $(Join-Path $DistDir 'SHA256SUMS-windows')"
Write-Output 'portable only: no signing or installer'
