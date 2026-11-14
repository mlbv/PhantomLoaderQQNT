#requires -Version 7.0
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding()]
param([Parameter(Mandatory)][string]$OutputDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-CommandOutput([string]$Program, [string[]]$Arguments) {
    $result = & $Program @Arguments
    if ($LASTEXITCODE) { throw "$Program failed with exit code $LASTEXITCODE." }
    return $result -join "`n"
}

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$profilePath = Join-Path $PSScriptRoot 'profile.json'
$profile = Get-Content -LiteralPath $profilePath -Raw | ConvertFrom-Json
$name = 'PhantomLoaderQQNT-' + $profile.platformVersion + '-QQNT-' + $profile.qqVersion + '-win-x64'
$metadata = Get-CommandOutput 'cargo' @('metadata', '--manifest-path', (Join-Path $root 'native/Cargo.toml'), '--no-deps', '--format-version', '1', '--locked') | ConvertFrom-Json
foreach ($member in $metadata.packages | Where-Object { $_.id -in $metadata.workspace_members }) {
    if ($member.version -cne $profile.platformVersion) { throw "Native workspace version differs from the release profile: $($member.name)" }
}
$file = $name + '.zip'
$path = Join-Path $OutputDirectory $file
$hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
$checksum = Get-Content -LiteralPath ($path + '.sha256') -Raw
if ($checksum -cne ($hash + '  ' + $file + "`n")) { throw "Archive checksum does not match: $file" }
$info = [ordered]@{
    schemaVersion = 1
    commit = Get-CommandOutput 'git' @('-C', $root, 'rev-parse', 'HEAD')
    ref = $env:GITHUB_REF
    run = if ($env:GITHUB_ACTIONS -eq 'true') { "$env:GITHUB_SERVER_URL/$env:GITHUB_REPOSITORY/actions/runs/$env:GITHUB_RUN_ID/attempts/$env:GITHUB_RUN_ATTEMPT" } else { $null }
    runnerImage = $env:ImageOS
    runnerImageVersion = $env:ImageVersion
    rust = Get-CommandOutput 'rustc' @('--version', '--verbose')
    cargo = Get-CommandOutput 'cargo' @('--version')
    node = Get-CommandOutput 'node' @('--version')
    powershell = $PSVersionTable.PSVersion.ToString()
    profileSha256 = (Get-FileHash -LiteralPath $profilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    archives = @([ordered]@{ name = $file; sha256 = $hash })
}
[IO.File]::WriteAllText((Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) 'build-info.json'),
    ($info | ConvertTo-Json -Depth 10) + "`n", [Text.UTF8Encoding]::new($false))
