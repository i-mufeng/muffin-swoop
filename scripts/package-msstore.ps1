#requires -Version 5.1
<#
.SYNOPSIS
Build a Microsoft Store-ready MSIX package for Swoop without installation or upload.
.DESCRIPTION
Packages the release binaries as an unsigned MSIX (desktop bridge / full-trust) using
MakeAppx.exe from the Windows SDK. The Microsoft Store re-signs uploaded packages, so no
certificate is required for submission; pass -PfxPath only when you want a locally
installable (sideload) test build.

The -PackageName / -Publisher defaults are the real Swoop Download Manager identity
reserved in Partner Center (MufengDev.SwoopDownloadManager); only override them if the
Store reservation changes. Bump -VersionOverride for every upload; Store versions must
strictly increase.
.EXAMPLE
pwsh -ExecutionPolicy Bypass -File scripts/package-msstore.ps1 -Profile release
.EXAMPLE
pwsh -File scripts/package-msstore.ps1 -VersionOverride 0.7.1.0 -PfxPath devcert.pfx
#>
[CmdletBinding()]
param(
    [ValidateSet('release', 'debug')]
    [string]$Profile = 'release',
    [string]$OutputDir = '',
    [string]$PackageName = 'MufengDev.SwoopDownloadManager',
    [string]$Publisher = 'CN=A1AF5AE8-6831-460B-AA8A-8BD04BC2F9B0',
    [string]$PublisherDisplayName = 'MufengDev',
    # Manifest DisplayName must match the app name reserved in Partner Center.
    [string]$DisplayName = 'Swoop Download Manager',
    [string]$VersionOverride = '',
    [string]$PfxPath = '',
    [string]$PfxPassword = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
    throw 'Run this script on Windows with the MSVC Rust toolchain.'
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$previousRustFlags = $env:RUSTFLAGS
# Cargo prints UTF-8, but PowerShell decodes native output with the console code page
# (GBK on Chinese Windows), which garbles crate descriptions and breaks the metadata JSON.
$previousOutputEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
$stageDir = $null
$lockDir = $null
Push-Location -LiteralPath $repoRoot
try {
    foreach ($tool in @('cargo', 'rustc', 'node', 'npm')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Required command is missing: $tool"
        }
    }
    # Locate the newest Windows SDK with MakeAppx.exe (x64 host tools).
    $programFilesX86 = ${env:ProgramFiles(x86)}
    if ([string]::IsNullOrWhiteSpace($programFilesX86)) { $programFilesX86 = 'C:\Program Files (x86)' }
    $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10\bin'
    $makeappx = $null
    $signtool = $null
    if (Test-Path -LiteralPath $kitsRoot -PathType Container) {
        $sdkVersions = @(Get-ChildItem -LiteralPath $kitsRoot -Directory -Filter '10.0.*' |
                Sort-Object { [version]($_.Name) } -Descending)
        foreach ($sdk in $sdkVersions) {
            $candidate = Join-Path $sdk.FullName 'x64\MakeAppx.exe'
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $makeappx = $candidate
                $signtool = Join-Path $sdk.FullName 'x64\signtool.exe'
                break
            }
        }
    }
    if (-not $makeappx) {
        throw "MakeAppx.exe not found under $kitsRoot. Install the Windows SDK (winget install Microsoft.WindowsSDK)."
    }

    $metadataText = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed: $LASTEXITCODE" }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $appPackage = @($metadata.packages | Where-Object { $_.name -eq 'swoop-app' })
    if ($appPackage.Count -ne 1) { throw 'Expected exactly one swoop-app package.' }
    $version = $appPackage[0].version
    if (-not [string]::IsNullOrWhiteSpace($VersionOverride)) { $version = $VersionOverride.Trim() }
    # MSIX/Store versions are strictly four 16-bit parts; 0.7.0 becomes 0.7.0.0.
    $versionParts = @($version -split '\.' | ForEach-Object { [int]$_ })
    if ($versionParts.Count -lt 3 -or $versionParts.Count -gt 4) {
        throw "Unexpected version '$version'; expected 3 or 4 numeric parts."
    }
    while ($versionParts.Count -lt 4) { $versionParts += 0 }
    foreach ($part in $versionParts) {
        if ($part -lt 0 -or $part -gt 65535) { throw "Version part out of 0-65535 range in '$version'." }
    }
    $msixVersion = $versionParts -join '.'

    $rustVersion = & rustc -vV
    if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed: $LASTEXITCODE" }
    $target = ($rustVersion | Where-Object { $_ -like 'host: *' }) -replace '^host: ', ''
    $archMap = @{ 'x86_64-pc-windows-msvc' = 'x64'; 'aarch64-pc-windows-msvc' = 'arm64' }
    if (-not $archMap.ContainsKey($target)) {
        throw "Unsupported native target: $target. Use the x64 or ARM64 MSVC toolchain."
    }
    $arch = $archMap[$target]

    # A static CRT avoids requiring an administrator-installed VC redistributable.
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
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Executable not found: $binary" }
    if (-not (Test-Path -LiteralPath $cliBinary -PathType Leaf)) { throw "CLI executable not found: $cliBinary" }

    if ([string]::IsNullOrWhiteSpace($OutputDir)) {
        $OutputDir = Join-Path $metadata.target_directory 'packages'
    }
    $null = New-Item -ItemType Directory -Path $OutputDir -Force
    $OutputDir = (Resolve-Path -LiteralPath $OutputDir).Path
    # NOTE: keep this name distinct from $PackageName (the Store identity); PowerShell
    # variables are case-insensitive and would otherwise clobber the parameter.
    $artifactName = "Swoop-$msixVersion-windows-$arch-Store"
    $msixPath = Join-Path $OutputDir "$artifactName.msix"
    $candidateLock = Join-Path $OutputDir ".$artifactName.lock"
    try {
        $null = New-Item -ItemType Directory -Path $candidateLock -ErrorAction Stop
    }
    catch {
        throw "Package output is locked; use another -OutputDir or inspect this lock: $candidateLock"
    }
    $lockDir = $candidateLock
    $outputs = @($msixPath, "$msixPath.sha256")

    # Stage the package payload: everything in this directory becomes package content.
    $stageDir = Join-Path $OutputDir ('.swoop-msix-' + [Guid]::NewGuid().ToString('N'))
    $assetsDir = Join-Path $stageDir 'Assets'
    $null = New-Item -ItemType Directory -Path $assetsDir
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stageDir 'swoop.exe')
    Copy-Item -LiteralPath $cliBinary -Destination (Join-Path $stageDir 'swoop-cli.exe')
    Copy-Item -LiteralPath 'assets/fonts/NOTICE.md' -Destination (Join-Path $stageDir 'FONT-NOTICE.md')
    [System.IO.File]::WriteAllText((Join-Path $stageDir 'swoop-channel.txt'), "msix-store`n")

    # Browser extension payload (same layout the NSIS installer ships).
    $extensionCrx = Join-Path $stageDir 'swoop-extension.crx'
    $extensionXpi = Join-Path $stageDir 'swoop-extension.xpi'
    $extensionInfo = Join-Path $stageDir 'swoop-extension-info.json'
    # Missing build tools or payloads must fail the build, never silently omit browser support.
    & {
        $extVersion = (Get-Content -LiteralPath (Join-Path $repoRoot 'extension/package.json') -Raw | ConvertFrom-Json).version
        Push-Location -LiteralPath (Join-Path $repoRoot 'extension')
        try {
            & npm run build:chromium
            if ($LASTEXITCODE -ne 0) { throw "extension build (chromium) failed: $LASTEXITCODE" }
            & npm run build:firefox
            if ($LASTEXITCODE -ne 0) { throw "extension build (firefox) failed: $LASTEXITCODE" }
            & npm run package:crx -- "--out=$(Join-Path $metadata.target_directory 'packages')"
            if ($LASTEXITCODE -ne 0) { throw "extension package:crx failed: $LASTEXITCODE" }
        }
        finally { Pop-Location }
        $extPackages = Join-Path $metadata.target_directory 'packages'
        Copy-Item -LiteralPath (Join-Path $extPackages "swoop-extension-chromium-$extVersion.crx") -Destination $extensionCrx
        Copy-Item -LiteralPath (Join-Path $extPackages "swoop-extension-firefox-$extVersion.xpi") -Destination $extensionXpi
        Copy-Item -LiteralPath (Join-Path $extPackages 'swoop-extension-info.json') -Destination $extensionInfo
        Copy-Item -LiteralPath (Join-Path $repoRoot 'extension/dist') -Destination (Join-Path $stageDir 'swoop-extension') -Recurse
        Write-Output "Extension payload staged (chromium + firefox, $extVersion)."
    }

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
version=$msixVersion
store_package_name=$PackageName
publisher=$Publisher
target=$target
profile=$Profile
source_revision=$revision
rustc=$($rustVersion[0])
binary_sha256=$binaryHash
cli_sha256=$cliHash
crt_linkage=static_requested
"@ | Set-Content -LiteralPath (Join-Path $stageDir 'build-info.txt') -Encoding UTF8

    # Tile/store logos: resize the brand artwork to the sizes the manifest references.
    Add-Type -AssemblyName System.Drawing
    $sourceIcon = Join-Path $repoRoot 'assets/brand/swoop/png/swoop-app-512.png'
    if (-not (Test-Path -LiteralPath $sourceIcon -PathType Leaf)) { throw "Brand icon missing: $sourceIcon" }
    $logoSizes = @{
        'StoreLogo.png'           = 50
        'Square44x44Logo.png'     = 44
        'Square150x150Logo.png'   = 150
    }
    foreach ($logo in $logoSizes.GetEnumerator()) {
        $src = [System.Drawing.Bitmap]::new($sourceIcon)
        try {
            $bmp = [System.Drawing.Bitmap]::new($logo.Value, $logo.Value, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
            try {
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                try {
                    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                    $g.DrawImage($src, 0, 0, $logo.Value, $logo.Value)
                } finally { $g.Dispose() }
                $bmp.Save((Join-Path $assetsDir $logo.Key), [System.Drawing.Imaging.ImageFormat]::Png)
            } finally { $bmp.Dispose() }
        } finally { $src.Dispose() }
    }

    # Materialize the manifest from the template with this build's identity.
    # Strip the template's guidance comment first so token replacement cannot touch it.
    $template = (Get-Content -LiteralPath (Join-Path $repoRoot 'scripts/windows-store/AppxManifest.xml') -Raw) -replace '(?s)<!--.*?-->', ''
    $manifest = $template.
        Replace('__PACKAGE_NAME__', $PackageName).
        Replace('__PUBLISHER__', $Publisher).
        Replace('__PUBLISHER_DISPLAY_NAME__', $PublisherDisplayName).
        Replace('__DISPLAY_NAME__', $DisplayName).
        Replace('__VERSION__', $msixVersion).
        Replace('__ARCH__', $arch)
    # No BOM: MakeAppx treats a UTF-8 BOM in the manifest as an error.
    [System.IO.File]::WriteAllText((Join-Path $stageDir 'AppxManifest.xml'), $manifest, (New-Object System.Text.UTF8Encoding $false))

    # Pack to a staged path first and only replace the previous package after success.
    $stagedMsix = "$stageDir.msix"
    & $makeappx pack /d $stageDir /p $stagedMsix /o
    if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed: $LASTEXITCODE" }
    if (-not (Test-Path -LiteralPath $stagedMsix -PathType Leaf)) { throw "MakeAppx produced no output: $stagedMsix" }

    if (-not [string]::IsNullOrWhiteSpace($PfxPath)) {
        if (-not (Test-Path -LiteralPath $PfxPath -PathType Leaf)) { throw "PFX not found: $PfxPath" }
        $signArgs = @('sign', '/fd', 'SHA256')
        if (-not [string]::IsNullOrWhiteSpace($PfxPassword)) { $signArgs += @('/p', $PfxPassword) }
        $signArgs += @('/a', $stagedMsix)
        & $signtool @signArgs
        if ($LASTEXITCODE -ne 0) { throw "signtool failed: $LASTEXITCODE" }
        Write-Output "Signed with $PfxPath (sideload-ready)."
    } else {
        Write-Output 'Unsigned package (Store re-signs submissions). Add -PfxPath for a locally installable build.'
    }

    foreach ($path in $outputs) {
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    Move-Item -LiteralPath $stagedMsix -Destination $msixPath
    $msixHash = (Get-FileHash -LiteralPath $msixPath -Algorithm SHA256).Hash.ToLowerInvariant()
    "$msixHash  $(Split-Path -Leaf $msixPath)" | Set-Content -LiteralPath "$msixPath.sha256" -Encoding ASCII
    Write-Output "MSIX: $msixPath"
    Write-Output "Checksum: $msixPath.sha256"
    if ($PackageName -eq 'Muffin.Swoop' -or $Publisher -eq 'CN=mufeng') {
        Write-Output 'NOTE: default identity placeholders in use. For a real Store submission pass the reserved -PackageName and the Partner Center -Publisher.'
    }
    Write-Output 'Store upload artifact only; no registration, install, or upload performed.'
}
finally {
    $env:RUSTFLAGS = $previousRustFlags
    [Console]::OutputEncoding = $previousOutputEncoding
    if ($stageDir -and (Test-Path -LiteralPath $stageDir)) {
        # This directory is created by this invocation and contains no user data.
        $resolvedStage = (Resolve-Path -LiteralPath $stageDir).Path
        if ((Split-Path -Parent $resolvedStage) -ne $OutputDir -or
            -not (Split-Path -Leaf $resolvedStage).StartsWith('.swoop-msix-')) {
            throw "Refusing to remove staging directory outside output: $resolvedStage"
        }
        Remove-Item -LiteralPath $stageDir -Recurse -Force
    }
    if ($lockDir -and (Test-Path -LiteralPath $lockDir)) {
        Remove-Item -LiteralPath $lockDir
    }
    Pop-Location
}
