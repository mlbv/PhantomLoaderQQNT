# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$QQRoot, [string]$InstallDirectory, [switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
try {
    Import-Module (Join-Path $PSScriptRoot 'Installer.Common.psm1') -Force -ErrorAction Stop
    Invoke-PlatformUninstall @PSBoundParameters
} catch {
    Write-Error $_ -ErrorAction Continue
    exit 1
}
