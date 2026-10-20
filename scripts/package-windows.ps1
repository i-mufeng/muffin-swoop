#requires -Version 5.1
<#
.SYNOPSIS
Build a native Windows Swoop zip and per-user NSIS installer without installation, signing, or upload.
.DESCRIPTION
Requires cargo-packager: cargo install cargo-packager --version 0.11.8 --locked
.EXAMPLE
pwsh -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Profile release
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
# Cargo prints UTF-8, but PowerShell decodes native output with the console code page
# (GBK on Chinese Windows), which garbles crate descriptions and breaks the metadata JSON.
$previousOutputEncoding = [Console]::OutputEncoding
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
$stageDir = $null
$lockDir = $null
Push-Location -LiteralPath $repoRoot
try {
    foreach ($tool in @('cargo', 'cargo-packager', 'rustc')) {
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
    $installer = "$packageDir-setup.exe"
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
    $outputs = @($packageDir, $archive, "$archive.sha256", $installer, "$installer.sha256")

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

    # Browser extension payload: build it (requires Node.js) and stage under stable file names
    # so swoop-cli can find the crx/xpi next to the executables after installation.
    $extensionCrx = Join-Path $stagedPackage 'swoop-extension.crx'
    $extensionXpi = Join-Path $stagedPackage 'swoop-extension.xpi'
    $extensionInfo = Join-Path $stagedPackage 'swoop-extension-info.json'
    if (Get-Command npm -ErrorAction SilentlyContinue) {
        $extVersion = (Get-Content -LiteralPath (Join-Path $repoRoot 'extension/package.json') -Raw | ConvertFrom-Json).version
        Push-Location -LiteralPath (Join-Path $repoRoot 'extension')
        try {
            & npm run build:chromium
            if ($LASTEXITCODE -ne 0) { throw "extension build (chromium) failed: $LASTEXITCODE" }
            & npm run build:firefox
            if ($LASTEXITCODE -ne 0) { throw "extension build (firefox) failed: $LASTEXITCODE" }
            & npm run package:crx
            if ($LASTEXITCODE -ne 0) { throw "extension package:crx failed: $LASTEXITCODE" }
        }
        finally { Pop-Location }
        $extPackages = Join-Path $metadata.target_directory 'packages'
        Copy-Item -LiteralPath (Join-Path $extPackages "swoop-extension-chromium-$extVersion.crx") -Destination $extensionCrx
        Copy-Item -LiteralPath (Join-Path $extPackages "swoop-extension-firefox-$extVersion.xpi") -Destination $extensionXpi
        Copy-Item -LiteralPath (Join-Path $extPackages 'swoop-extension-info.json') -Destination $extensionInfo
        Write-Output "Extension payload staged (chromium + firefox, $extVersion)."
    } else {
        Write-Output 'npm not found; the installer will ship without the browser extension payload.'
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

    # The per-user installer puts swoop.exe, swoop-cli.exe and build-info.txt side by side in
    # %LOCALAPPDATA%\Swoop, the layout swoop-updater replaces in place without elevation.
    $packagerDir = Join-Path $stageDir 'packager'
    $packagerOut = Join-Path $packagerDir 'out'
    $null = New-Item -ItemType Directory -Path $packagerOut
    # NSIS only loads BMP. The brand art is kept as PNG (scripts/make_installer_art.py) and
    # flattened to 24-bit BMP here; the template picks a variant by the dialog's real size.
    $artDir = Join-Path $packagerDir 'installer-art'
    $null = New-Item -ItemType Directory -Path $artDir
    Add-Type -AssemblyName System.Drawing
    Get-ChildItem -LiteralPath (Join-Path $repoRoot 'assets/brand/swoop/installer') -Filter '*.png' -File | ForEach-Object {
        $png = [System.Drawing.Bitmap]::new($_.FullName)
        try {
            $rect = [System.Drawing.Rectangle]::new(0, 0, $png.Width, $png.Height)
            $bmp = $png.Clone($rect, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
            try { $bmp.Save((Join-Path $artDir "$($_.BaseName).bmp"), [System.Drawing.Imaging.ImageFormat]::Bmp) }
            finally { $bmp.Dispose() }
        }
        finally { $png.Dispose() }
    }
    foreach ($kind in 'sidebar', 'header') {
        foreach ($scale in '100', '125', '150', '175', '200', '250') {
            if (-not (Test-Path -LiteralPath (Join-Path $artDir "$kind-$scale.bmp") -PathType Leaf)) {
                throw "Installer art missing: $kind-$scale.png"
            }
        }
    }
    # An explicit name keeps cargo-packager 0.11 from probing the config file path as a directory.
    $packagerResources = @(
        [ordered]@{ src = (Join-Path $stagedPackage 'build-info.txt'); target = 'build-info.txt' },
        [ordered]@{ src = (Join-Path $stagedPackage 'FONT-NOTICE.md'); target = 'FONT-NOTICE.md' }
    )
    if (Test-Path -LiteralPath $extensionCrx -PathType Leaf) {
        $packagerResources += [ordered]@{ src = $extensionCrx; target = 'swoop-extension.crx' }
        $packagerResources += [ordered]@{ src = $extensionXpi; target = 'swoop-extension.xpi' }
        $packagerResources += [ordered]@{ src = $extensionInfo; target = 'swoop-extension-info.json' }
    }
    $packagerConfig = [ordered]@{
        name         = 'swoop-app'
        productName  = 'Swoop'
        version      = $version
        identifier   = 'com.muffin.swoop'
        publisher    = 'mufeng'
        copyright    = 'Copyright (c) mufeng'
        category     = 'Utility'
        targetTriple = $target
        binariesDir  = $stagedPackage
        outDir       = $packagerOut
        binaries     = @(
            [ordered]@{ path = 'swoop'; main = $true },
            [ordered]@{ path = 'swoop-cli'; main = $false }
        )
        icons        = @((Join-Path $repoRoot 'assets/brand/swoop/swoop.ico'))
        resources    = $packagerResources
        nsis         = [ordered]@{
            installMode   = 'both'
            template      = (Join-Path $repoRoot 'scripts/windows-installer/installer.nsi')
            installerIcon = (Join-Path $repoRoot 'assets/brand/swoop/swoop.ico')
            sidebarImage  = (Join-Path $artDir 'sidebar-100.bmp')
            headerImage   = (Join-Path $artDir 'header-100.bmp')
            languages     = @('SimpChinese', 'English')
            # Offered as an unchecked option in the uninstaller; downloaded files are never touched.
            appdataPaths  = @('$APPDATA\com.muffin.swoop')
        }
    }
    $packagerConfigPath = Join-Path $packagerDir 'packager.json'
    # cargo-packager rejects a UTF-8 BOM, which Windows PowerShell's Set-Content -Encoding UTF8 writes.
    [System.IO.File]::WriteAllText($packagerConfigPath, ($packagerConfig | ConvertTo-Json -Depth 5), (New-Object System.Text.UTF8Encoding $false))
    & cargo packager --config $packagerConfigPath --formats nsis
    if ($LASTEXITCODE -ne 0) { throw "cargo packager failed: $LASTEXITCODE" }
    $setups = @(Get-ChildItem -LiteralPath $packagerOut -Filter '*-setup.exe' -File)
    if ($setups.Count -ne 1) { throw "Expected exactly one NSIS installer in $packagerOut" }
    $stagedInstaller = Join-Path $stageDir "$packageName-setup.exe"
    Move-Item -LiteralPath $setups[0].FullName -Destination $stagedInstaller
    $installerHash = (Get-FileHash -LiteralPath $stagedInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
    "$installerHash  $packageName-setup.exe" | Set-Content -LiteralPath "$stagedInstaller.sha256" -Encoding ASCII
    Remove-Item -LiteralPath $packagerDir -Recurse -Force

    # Build the complete zip before moving it into its final location.
    # Compress-Archive writes backslash entry names (invalid per the ZIP spec and rejected by the
    # updater's strict manifest checks), so create entries manually with forward slashes.
    $stagedArchive = Join-Path $stageDir "$packageName.zip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::Open($stagedArchive, 'Create')
    try {
        $rootName = Split-Path -Leaf $stagedPackage
        foreach ($item in Get-ChildItem -LiteralPath $stagedPackage -Recurse -Force) {
            $relative = ($item.FullName.Substring($stagedPackage.Length).TrimStart('\') -replace '\\', '/')
            if ($item.PSIsContainer) {
                $zip.CreateEntry("$rootName/$relative/") | Out-Null
            } else {
                [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $item.FullName, "$rootName/$relative", 'Optimal') | Out-Null
            }
        }
    } finally {
        $zip.Dispose()
    }
    $archiveHash = (Get-FileHash -LiteralPath $stagedArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    "$archiveHash  $packageName.zip" | Set-Content -LiteralPath "$stagedArchive.sha256" -Encoding ASCII
    # Replace an earlier package only after the new one is complete, so a failed build keeps it.
    foreach ($path in $outputs) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Recurse -Force
        }
    }
    Move-Item -LiteralPath $stagedPackage -Destination $packageDir
    Move-Item -LiteralPath $stagedArchive -Destination $archive
    Move-Item -LiteralPath "$stagedArchive.sha256" -Destination "$archive.sha256"
    Move-Item -LiteralPath $stagedInstaller -Destination $installer
    Move-Item -LiteralPath "$stagedInstaller.sha256" -Destination "$installer.sha256"
    Write-Output "Package: $packageDir"
    Write-Output "Archive: $archive"
    Write-Output "Checksum: $archive.sha256"
    Write-Output "Installer: $installer"
    Write-Output 'Local development package only; no Authenticode signature, install, launch, registry change, or upload performed.'
}
finally {
    $env:RUSTFLAGS = $previousRustFlags
    [Console]::OutputEncoding = $previousOutputEncoding
    if ($stageDir -and (Test-Path -LiteralPath $stageDir)) {
        # This directory is created by this invocation and contains no user data.
        Remove-Item -LiteralPath $stageDir -Recurse -Force
    }
    if ($lockDir -and (Test-Path -LiteralPath $lockDir)) {
        Remove-Item -LiteralPath $lockDir
    }
    Pop-Location
}
