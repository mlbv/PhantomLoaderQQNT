#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding()]
param(
    [string]$Package,
    [string]$Features,
    [string]$TargetDirectory,
    [switch]$Offline,
    [string]$ArchiveDirectory
)
foreach ($module in @('Microsoft.PowerShell.Management', 'Microsoft.PowerShell.Utility')) {
    Import-Module (Join-Path $PSHOME ('Modules/' + $module + '/' + $module + '.psd1'))
}
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'prepare-dependencies.ps1') -Offline:$Offline -ArchiveDirectory $ArchiveDirectory
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cargoHome = if ($env:CARGO_HOME) { [IO.Path]::GetFullPath($env:CARGO_HOME) } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { [IO.Path]::GetFullPath($env:RUSTUP_HOME) } else { Join-Path $env:USERPROFILE '.rustup' }
$separator = [char]31
$previous = $env:CARGO_ENCODED_RUSTFLAGS
$flags = @()
if ($previous) { $flags = @($previous -split $separator) }
elseif ($env:RUSTFLAGS) { $flags = @([regex]::Split($env:RUSTFLAGS.Trim(), '\s+')) }
foreach ($mapping in @(@($root, '.'), @($cargoHome, 'cargo'), @($rustupHome, 'rust'))) {
    # rustc source paths can use either Windows separator spelling.
    foreach ($prefix in @($mapping[0], $mapping[0].Replace('\', '/')) | Select-Object -Unique) {
        $flags += '--remap-path-prefix=' + $prefix + '=' + $mapping[1]
    }
}
$flags += '-Ctarget-feature=+crt-static'
$arguments = @('build', '--manifest-path', (Join-Path $PSScriptRoot 'Cargo.toml'), '--release', '--locked')
if ($Offline) { $arguments += '--offline' }
if ($Package) { $arguments += @('-p', $Package) }
if ($Features) { $arguments += @('--features', $Features) }
if ($TargetDirectory) { $arguments += @('--target-dir', $TargetDirectory) }
try {
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join $separator
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "Native Cargo build failed with exit code $LASTEXITCODE." }
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $previous
}
