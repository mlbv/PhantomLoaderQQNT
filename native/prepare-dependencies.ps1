#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding()]
param(
    [switch]$Offline,
    [string]$ArchiveDirectory
)
foreach ($module in @('Microsoft.PowerShell.Management', 'Microsoft.PowerShell.Utility')) {
    Import-Module (Join-Path $PSHOME ('Modules/' + $module + '/' + $module + '.psd1'))
}
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false)
$patchRoot = Join-Path $PSScriptRoot 'patches'
$buildRoot = Join-Path $PSScriptRoot 'build'
$dependencyRoot = Join-Path $buildRoot 'dependencies'
if ($ArchiveDirectory) {
    $archiveProvider = $null
    $archiveDrive = $null
    $ArchiveDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath(
        $ArchiveDirectory, [ref]$archiveProvider, [ref]$archiveDrive)
    if ($archiveProvider.Name -ne 'FileSystem') { throw 'ArchiveDirectory must be a filesystem path.' }
} else {
    $ArchiveDirectory = Join-Path $buildRoot 'dependency-cache'
}
$stateName = '.source-state.json'
$prepareSha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash.ToLowerInvariant()

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}
function Get-TextSha256([string]$Text) {
    $hash = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($hash.ComputeHash($utf8.GetBytes($Text))).Replace('-', '').ToLowerInvariant() }
    finally { $hash.Dispose() }
}
function Get-SourceFiles([string]$Root) {
    $paths = [string[]]@(Get-ChildItem -LiteralPath $Root -Recurse -File -Force |
        Where-Object { $_.FullName -cne (Join-Path $Root $stateName) } |
        ForEach-Object { $_.FullName.Substring($Root.Length + 1).Replace('\', '/') })
    [Array]::Sort($paths, [StringComparer]::Ordinal)
    foreach ($path in $paths) {
        [pscustomobject][ordered]@{ path = $path; sha256 = Get-Sha256 (Join-Path $Root $path) }
    }
}
function Get-TreeSha256($Files) {
    return Get-TextSha256 (($Files | ForEach-Object { $_.sha256 + '  ' + $_.path + "`n" }) -join '')
}
function Get-Archive($Dependency) {
    $archive = Join-Path $ArchiveDirectory ($Dependency.name + '-' + $Dependency.archive.sha256 + '.archive')
    if (-not [IO.File]::Exists($archive)) {
        if ($Offline) { throw "Missing offline dependency archive: $archive" }
        $download = Join-Path $ArchiveDirectory ('.download-' + [Guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($download)
        try {
            $downloadedArchive = Join-Path $download 'source.archive'
            Invoke-WebRequest -UseBasicParsing -Uri $Dependency.archive.url -OutFile $downloadedArchive
            if ((Get-Sha256 $downloadedArchive) -cne $Dependency.archive.sha256) {
                throw "Downloaded archive checksum differs: $($Dependency.name)"
            }
            [IO.File]::Move($downloadedArchive, $archive)
        } finally { [IO.Directory]::Delete($download, $true) }
    }
    if ((Get-Sha256 $archive) -cne $Dependency.archive.sha256) {
        throw "Cached archive checksum differs: $archive"
    }
    return $archive
}

$manifest = [IO.File]::ReadAllText((Join-Path $patchRoot 'dependencies.json'), $utf8) | ConvertFrom-Json
if ($manifest.schemaVersion -ne 1) { throw 'Unsupported dependency manifest schema.' }
[void][IO.Directory]::CreateDirectory($buildRoot)
[void][IO.Directory]::CreateDirectory($dependencyRoot)
[void][IO.Directory]::CreateDirectory($ArchiveDirectory)
$lock = [IO.File]::Open((Join-Path $buildRoot '.prepare-dependencies.lock'), [IO.FileMode]::OpenOrCreate,
    [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
try {
    foreach ($dependency in $manifest.dependencies) {
        $patches = @(foreach ($relative in $dependency.patches) {
            $path = [IO.Path]::GetFullPath((Join-Path $patchRoot $relative))
            [pscustomobject]@{ path = $path; sha256 = Get-Sha256 $path }
        })
        $inputs = $prepareSha256 + "`n" + ($dependency | ConvertTo-Json -Depth 20 -Compress) + "`n" +
            (($patches | ForEach-Object { $_.sha256 + "`n" }) -join '')
        $fingerprint = Get-TextSha256 $inputs
        $destination = Join-Path $dependencyRoot $dependency.name
        if ([IO.Directory]::Exists($destination)) {
            $state = [IO.File]::ReadAllText((Join-Path $destination $stateName), $utf8) | ConvertFrom-Json
            $files = @(Get-SourceFiles $destination)
            if ((Get-TreeSha256 $files) -cne $state.treeSha256 -or
                (Get-TreeSha256 $state.files) -cne $state.treeSha256) {
                throw "Prepared dependency sources were modified: $destination. Edit the checked-in patch instead; remove the generated directory before preparing it again."
            }
            if ($state.fingerprint -ceq $fingerprint) {
                Write-Host "Verified prepared dependency: $($dependency.name)"
                continue
            }
        }

        $archive = Get-Archive $dependency
        $stage = Join-Path $dependencyRoot ('.stage-' + $dependency.name + '-' + [Guid]::NewGuid().ToString('N'))
        [void][IO.Directory]::CreateDirectory($stage)
        try {
            $members = @($dependency.exports | ForEach-Object { $dependency.archive.rootPrefix + $_ })
            & tar -xf $archive -C $stage --strip-components=1 -- @members
            if ($LASTEXITCODE -ne 0) { throw "Dependency extraction failed: $($dependency.name)" }
            $previousCeiling = $env:GIT_CEILING_DIRECTORIES
            try {
                $env:GIT_CEILING_DIRECTORIES = $dependencyRoot
                foreach ($patch in $patches) {
                    & git -C $stage -c core.autocrlf=false apply --check --whitespace=error-all -p1 $patch.path
                    if ($LASTEXITCODE -ne 0) { throw "Dependency patch does not match its base: $($patch.path)" }
                    & git -C $stage -c core.autocrlf=false apply --whitespace=error-all -p1 $patch.path
                    if ($LASTEXITCODE -ne 0) { throw "Dependency patch failed: $($patch.path)" }
                }
            } finally { $env:GIT_CEILING_DIRECTORIES = $previousCeiling }
            $files = @(Get-SourceFiles $stage)
            $state = [ordered]@{
                fingerprint = $fingerprint
                prepareSha256 = $prepareSha256
                archiveSha256 = $dependency.archive.sha256
                patchSha256 = @($patches | ForEach-Object { $_.sha256 })
                treeSha256 = Get-TreeSha256 $files
                files = $files
            }
            [IO.File]::WriteAllText((Join-Path $stage $stateName), (($state | ConvertTo-Json -Depth 20) + "`n"), $utf8)
            if ([IO.Directory]::Exists($destination)) {
                $previous = Join-Path $dependencyRoot ('.previous-' + $dependency.name + '-' + [Guid]::NewGuid().ToString('N'))
                [IO.Directory]::Move($destination, $previous)
                try { [IO.Directory]::Move($stage, $destination) }
                catch { [IO.Directory]::Move($previous, $destination); throw }
                [IO.Directory]::Delete($previous, $true)
            } else { [IO.Directory]::Move($stage, $destination) }
            Write-Host "Prepared pinned dependency: $($dependency.name)"
        } finally {
            if ([IO.Directory]::Exists($stage)) { [IO.Directory]::Delete($stage, $true) }
        }
    }
} finally { $lock.Dispose() }
