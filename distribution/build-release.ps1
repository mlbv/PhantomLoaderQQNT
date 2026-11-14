#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding()]
param(
    [string]$LoaderRoot,
    [string]$NativeBuildDirectory,
    [string]$OutputDirectory
)
foreach ($module in @('Microsoft.PowerShell.Management', 'Microsoft.PowerShell.Utility')) {
    Import-Module (Join-Path $PSHOME ('Modules/' + $module + '/' + $module + '.psd1'))
}
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $LoaderRoot) { $LoaderRoot = Join-Path $PSScriptRoot '..' }
if (-not $NativeBuildDirectory) { $NativeBuildDirectory = Join-Path $LoaderRoot 'native\build\rust\release' }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $PSScriptRoot '..\dist' }

function Get-Absolute([string]$Path) {
    return [IO.Path]::GetFullPath($Path)
}
function Assert-PlainTree([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Release inputs cannot be reparse points: $Path"
    }
    if ($item.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Force) {
            Assert-PlainTree $child.FullName
        }
    }
}
function Assert-PlainPath([string]$Path) {
    $cursor = Get-Absolute $Path
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Release paths cannot traverse reparse points: $cursor"
            }
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
function Copy-Input([string]$Source, [string]$Relative) {
    Assert-PlainPath $Source
    Assert-PlainTree $Source
    $destination = Join-Path $stage $Relative
    if (Test-Path -LiteralPath $destination) { throw "Duplicate release input: $Relative" }
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($destination)) | Out-Null
    Copy-Item -LiteralPath $Source -Destination $destination -Recurse
}
function Write-Utf8([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function Write-Archive([string]$Root, [string]$Path) {
    Assert-PlainTree $Root
    $archive = [IO.Compression.ZipFile]::Open($Path, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($file in Get-ChildItem -LiteralPath $Root -Recurse -File -Force | Sort-Object FullName) {
            $relative = $file.FullName.Substring($Root.Length + 1).Replace('\', '/')
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $file.FullName, $relative,
                [IO.Compression.CompressionLevel]::Optimal)
        }
    } finally { $archive.Dispose() }
}
$created = $false
$stage = $null
$zip = $null
$hashFile = $null
try {
    $LoaderRoot = Get-Absolute $LoaderRoot
    $NativeBuildDirectory = Get-Absolute $NativeBuildDirectory
    $OutputDirectory = Get-Absolute $OutputDirectory
    $distributionRoot = Join-Path $LoaderRoot 'distribution'
    Assert-PlainPath $LoaderRoot
    Assert-PlainPath $NativeBuildDirectory
    Assert-PlainPath $OutputDirectory
    $profilePath = Join-Path $distributionRoot 'profile.json'
    Assert-PlainTree $profilePath
    $profile = Get-Content -LiteralPath $profilePath -Raw | ConvertFrom-Json
    $loader = Get-Content -LiteralPath (Join-Path $LoaderRoot 'package.json') -Raw | ConvertFrom-Json
    if ($profile.platformVersion -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$' -or $profile.qqVersion -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+-[0-9]+$') {
        throw 'Release profile requires exact platform and QQNT versions.'
    }
    if ($loader.version -cne $profile.loaderVersion -or $loader.main -cne './src/main.js' -or
        $loader.compatibility.liteloader -cne $profile.liteloaderCompatibilityVersion -or $loader.license -cne 'LGPL-3.0-or-later') {
        throw 'Loader version, license or LiteLoader compatibility does not match the distribution contract.'
    }
    $injector = Join-Path $NativeBuildDirectory 'phantomloader.exe'
    $payload = Join-Path $NativeBuildDirectory 'phantomloader_payload.dll'
    Assert-PlainTree $injector
    Assert-PlainTree $payload
    $profileHash = (Get-FileHash -LiteralPath $profilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $injectorText = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($injector))
    if (-not $injectorText.Contains($profileHash)) {
        throw 'The native launcher does not contain this exact profile digest. Reconfigure and rebuild it.'
    }
    $injectorWide = [Text.Encoding]::Unicode.GetString([IO.File]::ReadAllBytes($injector))
    foreach ($required in @('-reporter', '-capture-directory', '-ProcessStartedFiletime', '-ProcessExitedFiletime')) {
        if (-not $injectorWide.Contains($required) -and -not $injectorText.Contains($required)) { throw 'The native launcher does not support the required incident monitor contract. Rebuild it.' }
    }
    $payloadText = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($payload))
    foreach ($required in @($profileHash, 'armDlVmCapture', 'dlVmCaptureStatus', 'targetProfileSha256', 'platformVersion')) {
        if (-not $payloadText.Contains($required)) {
            throw 'The native payload does not match the required platform/capture contract. Reconfigure and rebuild it.'
        }
    }
    foreach ($fixtureExport in @('InitializePayloadRuntime', 'InitializeDlVmCapture', 'InstallDlVmCaptureTargets', 'DisposeNativeHooks')) {
        if ($payloadText.Contains($fixtureExport)) { throw 'A fixture-api payload cannot be packaged for release.' }
    }
    $compiledHash = & $injector -profile-hash
    if ($LASTEXITCODE -ne 0 -or ([string]$compiledHash).Trim() -cne $profileHash) {
        throw 'The native launcher was not built with this exact distribution profile. Reconfigure and rebuild it.'
    }
    foreach ($source in @('LICENSES', 'src', 'distribution', 'docs', 'native/crates', 'native/patches', 'native/licenses' |
            ForEach-Object { Join-Path $LoaderRoot $_ }) + @($NativeBuildDirectory)) {
        if ($OutputDirectory.Equals($source, [StringComparison]::OrdinalIgnoreCase) -or
            $OutputDirectory.StartsWith($source.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "Release output must be outside input directories: $source"
        }
    }
    $name = 'PhantomLoaderQQNT-' + $profile.platformVersion + '-QQNT-' + $profile.qqVersion + '-win-x64'
    [IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
    $stage = Join-Path $OutputDirectory $name
    $zip = $stage + '.zip'
    $hashFile = $zip + '.sha256'
    foreach ($output in @($stage, $zip, $hashFile)) {
        if (Test-Path -LiteralPath $output) { throw "Release output already exists: $output" }
    }
    [IO.Directory]::CreateDirectory($stage) | Out-Null
    $created = $true
    $entryFiles = @('Install.ps1', 'Uninstall.ps1',
        'Launch.ps1', 'Installer.Common.psm1', 'Incident.Common.psm1', 'Report-Capture.ps1', 'README.md', 'profile.json',
        'Install.cmd', 'Uninstall.cmd', 'Launch.cmd')
    foreach ($name in $entryFiles) { Copy-Input (Join-Path $LoaderRoot ('distribution/' + $name)) $name }
    Copy-Input $injector 'payload\phantomloader.exe'
    Copy-Input $payload 'payload\phantomloader_payload.dll'
    Copy-Input (Join-Path $LoaderRoot 'package.json') 'loader\package.json'
    Copy-Input (Join-Path $LoaderRoot 'distribution/profile.json') 'loader\distribution\profile.json'
    Copy-Input (Join-Path $LoaderRoot 'LICENSE') 'loader\LICENSE'
    $runtimeFiles = @(
        'src/main.js',
        'src/preload.js',
        'src/renderer.js',
        'src/common/static/config.json',
        'src/common/static/liteloader.json',
        'src/common/static/default.png',
        'src/common/static/default.svg',
        'src/main/api.js',
        'src/main/capture.js',
        'src/main/hook.js',
        'src/main/interception.js',
        'src/main/loader.js',
        'src/main/runtime.js',
        'src/main/store.js',
        'src/main/state.js',
        'src/main/transport.js',
        'src/preload/api.js',
        'src/preload/module.js',
        'src/preload/runtime.js',
        'src/renderer/api.js',
        'src/renderer/hook.js',
        'src/renderer/runtime.js',
        'src/renderer/components/element.js',
        'src/renderer/components/renderer.js',
        'src/renderer/components/elements/button.js',
        'src/renderer/components/elements/divider.js',
        'src/renderer/components/elements/item.js',
        'src/renderer/components/elements/link.js',
        'src/renderer/components/elements/list.js',
        'src/renderer/components/elements/modal.js',
        'src/renderer/components/elements/option.js',
        'src/renderer/components/elements/panel.js',
        'src/renderer/components/elements/section.js',
        'src/renderer/components/elements/select.js',
        'src/renderer/components/elements/switch.js',
        'src/renderer/components/elements/text.js',
        'src/renderer/settings/renderer.js',
        'src/renderer/settings/style.css',
        'src/renderer/settings/view.html',
        'src/renderer/triggers/renderer.js',
        'src/renderer/triggers/selector/search.js',
        'src/renderer/triggers/selector/setting.js',
        'src/renderer/triggers/selector/title.js',
        'src/renderer/triggers/selector/update.js'
    )
    foreach ($relative in $runtimeFiles) { Copy-Input (Join-Path $LoaderRoot $relative) ('loader/' + $relative) }
    Copy-Input (Join-Path $LoaderRoot 'LICENSES/CC-BY-4.0.txt') 'licenses/CC-BY-4.0.txt'
    Copy-Input (Join-Path $LoaderRoot 'LICENSES/GPL-3.0-only.txt') 'licenses/GPL-3.0.txt'
    Copy-Input (Join-Path $LoaderRoot 'LICENSES/LGPL-3.0-or-later.txt') 'licenses/LGPL-3.0.txt'
    Copy-Input (Join-Path $LoaderRoot 'LICENSES/LiteLoaderQQNT-MIT.txt') 'licenses/LiteLoaderQQNT-MIT.txt'
    Copy-Input (Join-Path $LoaderRoot 'native/build/dependencies/neohook/LICENSE-MIT') 'licenses/NeoHook-MIT.txt'
    foreach ($name in @('iced-x86-MIT.txt', 'windows-sys-MIT.txt', 'Rust-Third-Party.txt')) {
        Copy-Input (Join-Path $LoaderRoot ('native/licenses/' + $name)) ('licenses/' + $name)
    }

    if ((Get-FileHash -LiteralPath (Join-Path $stage 'profile.json') -Algorithm SHA256).Hash.ToLowerInvariant() -cne $profileHash) {
        throw 'The distribution profile changed while packaging.'
    }

    $rows = @(foreach ($file in Get-ChildItem -LiteralPath $stage -Recurse -File -Force | Sort-Object FullName) {
        $relative = $file.FullName.Substring($stage.Length + 1).Replace('\', '/')
        [ordered]@{
            path = $relative
            bytes = $file.Length
            sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    })
    $manifest = [ordered]@{
        schemaVersion = 1
        bundleVersion = $profile.platformVersion
        qqVersion = $profile.qqVersion
        loaderVersion = $profile.loaderVersion
        liteloaderCompatibilityVersion = $profile.liteloaderCompatibilityVersion
        nativeApiVersion = $profile.nativeApiVersion
        files = $rows
    }
    Write-Utf8 (Join-Path $stage 'bundle.json') (($manifest | ConvertTo-Json -Depth 20) + "`n")

    $installer = Import-Module (Join-Path $stage 'Installer.Common.psm1') -Force -PassThru
    try { & $installer { param($Root) [void](Get-PlatformBundle $Root) } $stage }
    finally { Remove-Module $installer -Force }

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    Write-Archive $stage $zip
    $zipHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Utf8 $hashFile ($zipHash + '  ' + [IO.Path]::GetFileName($zip) + "`n")
    Write-Host "Release directory: $stage"
    Write-Host "Archive: $zip"
    Write-Host "SHA-256: $zipHash"
} catch {
    if ($created) {
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -Confirm:$false }
        if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force -Confirm:$false }
        if (Test-Path -LiteralPath $hashFile) { Remove-Item -LiteralPath $hashFile -Force -Confirm:$false }
    }
    throw
}
