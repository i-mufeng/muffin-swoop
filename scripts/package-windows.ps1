#requires -Version 5.1
<#
.SYNOPSIS
Build a native Windows Swoop zip without installation, signing, or upload.
.EXAMPLE
powershell -File scripts/package-windows.ps1 -Profile release
.EXAMPLE
pwsh -File scripts/package-windows.ps1 -Profile debug -OutputDir "target/desktop packages"
#>
[CmdletBinding()]
param(
    [ValidateSet('release', 'debug')]
    [string]$Profile = 'release',
    [string]$OutputDir = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
    throw 'Run this script on Windows with the MSVC Rust toolchain.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$previousRustFlags = $env:RUSTFLAGS
$stageDir = $null
$lockDir = $null
Push-Location -LiteralPath $repoRoot
try {
    foreach ($tool in @('cargo', 'rustc')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Required command is missing: $tool"
        }
    }
    $metadataText = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed: $LASTEXITCODE" }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $appPackage = @($metadata.packages | Where-Object { $_.name -eq 'swoop-app' })
    if ($appPackage.Count -ne 1) { throw 'Expected exactly one swoop-app package.' }
    $version = $appPackage[0].version
    if ($version -notmatch '^\d+\.\d+\.\d+([+-][A-Za-z0-9.+-]+)?$') {
        throw "Unexpected Cargo package version: $version"
    }
    $rustVersion = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed: $LASTEXITCODE" }
    $target = ($rustVersion | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    if ($target -notmatch '^(x86_64|aarch64)-pc-windows-msvc$') {
        throw "Unsupported native target: $target. Use the x64 or ARM64 MSVC toolchain."
    }
    $arch = $target.Split('-')[0]

    # A static CRT avoids requiring an administrator-installed VC redistributable.
    # An explicit target keeps these flags off host build scripts/proc macros.
    if ($env:CARGO_ENCODED_RUSTFLAGS) {
        throw 'CARGO_ENCODED_RUSTFLAGS overrides RUSTFLAGS; clear it before packaging a static-CRT build.'
    }
    if ($previousRustFlags -notmatch '(^|\s)-C\s+target-feature=\+crt-static(\s|$)') {
        $env:RUSTFLAGS = "$previousRustFlags -C target-feature=+crt-static".Trim()
    }
    $buildArgs = @('build', '--locked', '--package', 'swoop-app', '--bins', '--target', $target)
    if ($Profile -eq 'release') { $buildArgs += '--release' }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $LASTEXITCODE" }

    $binary = Join-Path $metadata.target_directory "$target/$Profile/swoop.exe"
    $cliBinary = Join-Path $metadata.target_directory "$target/$Profile/swoop-cli.exe"
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "Executable not found: $binary"
    }
    if (-not (Test-Path -LiteralPath $cliBinary -PathType Leaf)) {
        throw "CLI executable not found: $cliBinary"
    }
    if ([string]::IsNullOrWhiteSpace($OutputDir)) {
        $OutputDir = Join-Path $metadata.target_directory 'packages'
    }
    $null = New-Item -ItemType Directory -Path $OutputDir -Force
    $OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
    $packageName = "Swoop-$version-windows-$arch-$Profile"
    $packageDir = Join-Path $OutputDir $packageName
    $archive = "$packageDir.zip"
    # Directory creation without -Force is an exclusive lock for this package name.
    # Set lockDir only after acquisition, so finally never removes another run's lock.
    $candidateLock = Join-Path $OutputDir ".$packageName.lock"
    try {
        $null = New-Item -ItemType Directory -Path $candidateLock -ErrorAction Stop
    }
    catch {
        throw "Package output is locked; use another -OutputDir or inspect this lock: $candidateLock"
    }
    $lockDir = $candidateLock
    foreach ($path in @($packageDir, $archive, "$archive.sha256")) {
        if (Test-Path -LiteralPath $path) {
            throw "Output already exists; use another -OutputDir: $path"
        }
    }

    $stageDir = Join-Path $OutputDir ('.swoop-package-' + [Guid]::NewGuid().ToString('N'))
    $stagedPackage = Join-Path $stageDir $packageName
    $null = New-Item -ItemType Directory -Path $stagedPackage
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stagedPackage 'swoop.exe')
    Copy-Item -LiteralPath $cliBinary -Destination (Join-Path $stagedPackage 'swoop-cli.exe')
    Copy-Item -LiteralPath 'README.md' -Destination $stagedPackage
    $docsDir = Join-Path $stagedPackage 'docs'
    $mediaDocsDir = Join-Path $stagedPackage 'crates/media'
    $fontDocsDir = Join-Path $stagedPackage 'assets/fonts'
    $null = New-Item -ItemType Directory -Path $docsDir, $mediaDocsDir, $fontDocsDir
    Get-ChildItem -LiteralPath 'docs' -Filter '*.md' -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $docsDir
    }
    Copy-Item -LiteralPath 'crates/media/README.md' -Destination $mediaDocsDir
    Copy-Item -LiteralPath 'assets/fonts/NOTICE.md' -Destination $fontDocsDir
    Copy-Item -LiteralPath 'docs/desktop-validation.md' -Destination $stagedPackage
    Copy-Item -LiteralPath 'docs/cli.md' -Destination $stagedPackage
    Copy-Item -LiteralPath 'assets/fonts/NOTICE.md' -Destination (Join-Path $stagedPackage 'FONT-NOTICE.md')
    $binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    $cliHash = (Get-FileHash -LiteralPath $cliBinary -Algorithm SHA256).Hash.ToLowerInvariant()
    $revision = 'unknown'
    if (Get-Command git -ErrorAction SilentlyContinue) {
        $gitRevision = & git rev-parse --verify HEAD 2>$null
        if ($LASTEXITCODE -eq 0) { $revision = $gitRevision }
    }
    @"
name=Swoop
application_id=com.muffin.swoop
version=$version
target=$target
profile=$Profile
source_revision=$revision
source_matches_binary=not_verified
rustc=$($rustVersion[0])
binary_sha256=$binaryHash
cli_sha256=$cliHash
crt_linkage=static_requested
authenticode_signature=none
"@ | Set-Content -LiteralPath (Join-Path $stagedPackage 'build-info.txt') -Encoding UTF8

    # Build the complete zip before moving it into its final location.
    $stagedArchive = Join-Path $stageDir "$packageName.zip"
    Compress-Archive -LiteralPath $stagedPackage -DestinationPath $stagedArchive -CompressionLevel Optimal
    $archiveHash = (Get-FileHash -LiteralPath $stagedArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$archiveHash  $packageName.zip" | Set-Content -LiteralPath "$stagedArchive.sha256" -Encoding ASCII
    Move-Item -LiteralPath $stagedPackage -Destination $packageDir
    Move-Item -LiteralPath $stagedArchive -Destination $archive
    Move-Item -LiteralPath "$stagedArchive.sha256" -Destination "$archive.sha256"
    Write-Output "Package: $packageDir"
    Write-Output "Archive: $archive"
    Write-Output "Checksum: $archive.sha256"
    Write-Output 'Local development package only; no Authenticode signature, install, launch, registry change, or upload performed.'
}
finally {
    $env:RUSTFLAGS = $previousRustFlags
    if ($stageDir -and (Test-Path -LiteralPath $stageDir)) {
        # This directory is created by this invocation and contains no user data.
        Remove-Item -LiteralPath $stageDir -Recurse -Force
    }
    if ($lockDir -and (Test-Path -LiteralPath $lockDir)) {
        Remove-Item -LiteralPath $lockDir
    }
    Pop-Location
}
