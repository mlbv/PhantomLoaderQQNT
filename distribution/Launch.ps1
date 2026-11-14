# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$InstallDirectory, [switch]$CheckOnly, [switch]$PauseOnError, [switch]$ShowErrorDialog)
if (-not $InstallDirectory) { $InstallDirectory = $PSScriptRoot }
$ErrorActionPreference = 'Stop'
try {
    Import-Module (Join-Path $PSScriptRoot 'Installer.Common.psm1') -Force -ErrorAction Stop
    Invoke-PlatformLaunch -InstallDirectory $InstallDirectory -CheckOnly:$CheckOnly -WhatIf:$WhatIfPreference
} catch {
    Write-Error $_ -ErrorAction Continue
    if ($ShowErrorDialog -and -not $CheckOnly -and -not $WhatIfPreference) {
        Add-Type -AssemblyName System.Windows.Forms
        [void][Windows.Forms.MessageBox]::Show($_.Exception.Message, 'PhantomLoaderQQNT launch failed', [Windows.Forms.MessageBoxButtons]::OK, [Windows.Forms.MessageBoxIcon]::Error)
    } elseif ($PauseOnError -and -not $CheckOnly -and -not $WhatIfPreference) {
        [void](Read-Host 'Launch failed. Press Enter to close this window')
    }
    exit 1
}
