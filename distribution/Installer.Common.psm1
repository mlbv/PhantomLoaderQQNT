#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$script:Utf8 = New-Object System.Text.UTF8Encoding($false)
$script:BootstrapMain = './app_launcher/PhantomLoaderQQNT.js'
$script:NativeCapture = ([IO.File]::ReadAllText((Join-Path $PSScriptRoot 'profile.json'), $script:Utf8) | ConvertFrom-Json).native.capture
$script:PublicFiles = @('Installer.Common.psm1', 'Incident.Common.psm1', 'Report-Capture.ps1', 'Install.ps1', 'Uninstall.ps1', 'Launch.ps1', 'README.md', 'Install.cmd', 'Uninstall.cmd', 'Launch.cmd', 'profile.json')

function Get-AbsolutePath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not [IO.Path]::IsPathRooted($Path)) { throw "An absolute path is required: $Path" }
    $full = [IO.Path]::GetFullPath($Path)
    if ($full -eq [IO.Path]::GetPathRoot($full)) { throw "A drive root cannot be used: $Path" }
    return $full.TrimEnd('\', '/')
}

function Test-PathWithin {
    param([string]$Path, [string]$Root)
    return $Path.Equals($Root, [StringComparison]::OrdinalIgnoreCase) -or $Path.StartsWith($Root.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoReparsePoint {
    param([string]$Path)
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse points are not supported: $cursor" }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
}

function Join-SafePath {
    param([string]$Root, [string]$Relative)
    if ([string]::IsNullOrWhiteSpace($Relative) -or [IO.Path]::IsPathRooted($Relative)) { throw "Invalid relative path: $Relative" }
    $parts = $Relative -split '[\\/]'
    foreach ($part in $parts) {
        if ($part -eq '' -or $part -eq '.' -or $part -eq '..' -or $part -match '[<>:"|?*\x00-\x1f]' -or $part -match '[. ]$' -or $part -match '^(?i:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)') { throw "Unsafe relative path: $Relative" }
    }
    $path = [IO.Path]::GetFullPath([IO.Path]::Combine($Root, ($parts -join [IO.Path]::DirectorySeparatorChar)))
    if (-not (Test-PathWithin $path $Root) -or $path -eq $Root) { throw "Path escapes its root: $Relative" }
    Assert-NoReparsePoint $path
    return $path
}

function Read-JsonFile {
    param([string]$Path)
    return ([IO.File]::ReadAllText($Path, $script:Utf8) | ConvertFrom-Json)
}

function Write-JsonFile {
    param([string]$Path, $Value)
    [IO.File]::WriteAllText($Path, (($Value | ConvertTo-Json -Depth 20) + [Environment]::NewLine), $script:Utf8)
}

function Get-FileFingerprint {
    param([string]$Path, [string]$Relative)
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer) { throw "Expected a file: $Path" }
    return [pscustomobject]@{ path = $Relative; bytes = $item.Length; sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
}

function Assert-FileFingerprint {
    param([string]$Root, $Entry)
    if ($Entry.bytes -lt 0 -or [string]$Entry.sha256 -cnotmatch '^[a-f0-9]{64}$') { throw "Invalid fingerprint: $($Entry.path)" }
    $path = Join-SafePath $Root $Entry.path
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Required file is missing: $path" }
    $actual = Get-FileFingerprint $path $Entry.path
    if ($actual.bytes -ne $Entry.bytes -or $actual.sha256 -cne $Entry.sha256) { throw "File fingerprint mismatch: $path" }
}

function Get-RelativeFiles {
    param([string]$Root)
    $result = New-Object 'System.Collections.Generic.List[string]'
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $pending.Push($Root)
    while ($pending.Count -gt 0) {
        foreach ($item in Get-ChildItem -LiteralPath $pending.Pop() -Force) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse points are not supported: $($item.FullName)" }
            if ($item.PSIsContainer) { $pending.Push($item.FullName) }
            else { $result.Add($item.FullName.Substring($Root.Length + 1).Replace('\', '/')) }
        }
    }
    return $result.ToArray()
}

function Assert-ExactFileSet {
    param([string[]]$Actual, [string[]]$Expected, [string]$Description)
    $seen = @{}
    foreach ($path in $Expected) {
        if ($seen.ContainsKey($path)) { throw "Duplicate file in $Description inventory: $path" }
        $seen[$path] = $true
    }
    if (@($Actual).Count -ne @($Expected).Count) { throw "$Description file inventory differs from the expected set." }
    foreach ($path in $Actual) {
        if (-not $seen.ContainsKey($path)) { throw "Unexpected file in ${Description}: $path" }
    }
}

function Get-PlatformBundle {
    param([string]$Directory, [switch]$Installed)
    $root = Get-AbsolutePath $Directory
    Assert-NoReparsePoint $root
    $manifest = Read-JsonFile (Join-SafePath $root 'bundle.json')
    if ($manifest.schemaVersion -ne 1 -or $manifest.bundleVersion -isnot [string] -or
        $manifest.bundleVersion -cnotmatch '\A(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\z' -or
        $manifest.loaderVersion -isnot [string]) { throw 'Unsupported bundle schema or version.' }
    $profile = Read-JsonFile (Join-SafePath $root 'profile.json')
    if ($profile.platformVersion -isnot [string] -or $manifest.bundleVersion -cne $profile.platformVersion) { throw 'Bundle/profile mismatch: platformVersion' }
    if ($profile.schemaVersion -ne 1 -or $profile.qqVersion -cne '9.9.37-54395' -or [string]$profile.buildVersion -cne '54395' -or $profile.platform -cne 'win32' -or $profile.architecture -cne 'x64' -or $profile.loaderVersion -isnot [string] -or $profile.loaderVersion -cne $profile.platformVersion -or $profile.liteloaderCompatibilityVersion -cne '5.2.1' -or $profile.nativeApiVersion -ne 1) { throw 'Unsupported QQNT platform profile.' }
    foreach ($key in @('qqVersion', 'loaderVersion', 'liteloaderCompatibilityVersion', 'nativeApiVersion')) {
        if ($manifest.$key -cne $profile.$key) { throw "Bundle/profile mismatch: $key" }
    }
    $prefix = 'versions/' + $profile.qqVersion + '/'
    $criticalPaths = @('QQ.exe', ($prefix + 'QQNT.dll'), ($prefix + 'resources/app/wrapper.node'), ($prefix + 'resources/app/application.asar'), ($prefix + 'resources/app/major.node'))
    Assert-ExactFileSet @($profile.files | ForEach-Object { $_.path }) $criticalPaths 'Profile'
    if ($profile.originalPackage.path -cne ($prefix + 'resources/app/package.json') -or $profile.originalPackage.main -cne './application.asar/app_launcher/index.js') { throw 'Unsupported original package location or entry point.' }
    $expected = New-Object 'System.Collections.Generic.List[string]'
    foreach ($entry in $manifest.files) {
        if ($entry.path -cne ([string]$entry.path).Replace('\', '/')) { throw "Bundle paths must use forward slashes: $($entry.path)" }
        $allowed = $script:PublicFiles -ccontains $entry.path
        $allowed = $allowed -or $entry.path -cmatch '^(?:payload/(?:phantomloader\.exe|phantomloader_payload\.dll)|loader/(?:package\.json|LICENSE|distribution/profile\.json|src/.+)|licenses/[^/]+\.txt)$'
        if (-not $allowed) { throw "Unsupported bundle asset: $($entry.path)" }
        Assert-FileFingerprint $root $entry
        $expected.Add($entry.path)
    }
    $required = $script:PublicFiles + @('payload/phantomloader.exe', 'payload/phantomloader_payload.dll', 'loader/package.json', 'loader/LICENSE', 'loader/src/main.js', 'loader/src/main/capture.js', 'loader/distribution/profile.json')
    foreach ($path in $required) {
        if (-not $expected.Contains($path)) { throw "Bundle manifest omits required asset: $path" }
    }
    $canonicalProfile = Get-FileFingerprint (Join-SafePath $root 'profile.json') 'profile.json'
    $runtimeProfile = Get-FileFingerprint (Join-SafePath $root 'loader/distribution/profile.json') 'loader/distribution/profile.json'
    if ($canonicalProfile.sha256 -cne $runtimeProfile.sha256) { throw 'Loader runtime profile differs from the canonical bundle profile.' }
    $expected.Add('bundle.json')
    if ($Installed) { $expected.Add('state.json'); $expected.Add('original-package.bin') }
    Assert-ExactFileSet @(Get-RelativeFiles $root) $expected.ToArray() 'Bundle'
    $loader = Read-JsonFile (Join-SafePath $root 'loader/package.json')
    if ($loader.version -cne $profile.loaderVersion -or $loader.compatibility.liteloader -cne $profile.liteloaderCompatibilityVersion -or $loader.main -cne './src/main.js') { throw 'Bundled loader version/entry point mismatch.' }
    return [pscustomobject]@{ Root = $root; Manifest = $manifest; Profile = $profile }
}

function Resolve-PlatformQQRoot {
    param([string]$QQRoot)
    if ($QQRoot) { return Get-AbsolutePath $QQRoot }
    $candidates = New-Object 'System.Collections.Generic.List[string]'
    foreach ($key in @('HKCU:\Software\Tencent\QQNT', 'HKLM:\SOFTWARE\Tencent\QQNT', 'HKLM:\SOFTWARE\WOW6432Node\Tencent\QQNT')) {
        if (Test-Path -LiteralPath $key) {
            $properties = Get-ItemProperty -LiteralPath $key
            foreach ($name in @('Install', 'InstallPath', 'InstallLocation')) {
                if ($properties.PSObject.Properties.Name -contains $name -and $properties.$name) { $candidates.Add((Get-AbsolutePath $properties.$name)) }
            }
        }
    }
    if ($env:ProgramFiles) { $candidates.Add((Join-Path $env:ProgramFiles 'Tencent\QQNT')) }
    $found = @($candidates | Select-Object -Unique | Where-Object { Test-Path -LiteralPath (Join-Path $_ 'QQ.exe') -PathType Leaf })
    if ($found.Count -ne 1) { throw 'QQNT location is missing or ambiguous. Supply -QQRoot with the exact installation directory.' }
    return Get-AbsolutePath $found[0]
}

function Get-PlatformLocations {
    param([string]$QQRoot, [string]$InstallDirectory, [string]$ProfileDirectory, [string]$DesktopDirectory)
    $root = Resolve-PlatformQQRoot $QQRoot
    if (-not $InstallDirectory) { $InstallDirectory = Join-Path $root 'PhantomLoaderQQNT' }
    if (-not $ProfileDirectory) {
        if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is unavailable. Supply -ProfileDirectory.' }
        $ProfileDirectory = Join-Path $env:LOCALAPPDATA 'PhantomLoaderQQNT\profile'
    }
    if (-not $DesktopDirectory) { $DesktopDirectory = [Environment]::GetFolderPath('Desktop') }
    $install = Get-AbsolutePath $InstallDirectory
    $profile = Get-AbsolutePath $ProfileDirectory
    $desktop = Get-AbsolutePath $DesktopDirectory
    $captures = Join-SafePath $profile 'data/dlvm_capture/captures'
    if ($captures.Length -gt $script:NativeCapture.maxDirectoryCodeUnits) { throw "The capture output directory must contain at most $($script:NativeCapture.maxDirectoryCodeUnits) UTF-16 code units. Choose a shorter -ProfileDirectory." }
    foreach ($path in @($root, $install, $profile, $desktop)) { Assert-NoReparsePoint $path }
    if ((Test-PathWithin $root $install) -or (Test-PathWithin $install (Join-Path $root 'versions')) -or (Test-PathWithin $profile $root) -or (Test-PathWithin $root $profile) -or (Test-PathWithin $profile $install) -or (Test-PathWithin $install $profile) -or (Test-PathWithin $desktop $install)) { throw 'QQ, install, profile and desktop directories overlap in an unsafe way.' }
    return [pscustomobject]@{ QQRoot = $root; InstallDirectory = $install; ProfileDirectory = $profile; DesktopDirectory = $desktop }
}

function Get-PlatformQQProcesses {
    return @(Get-Process -Name QQ, QQNT -ErrorAction SilentlyContinue)
}

function Assert-QQStopped {
    if (@(Get-PlatformQQProcesses).Count -gt 0) { throw 'QQ is running. Close QQ before changing this installation.' }
}

function Read-QQSelector {
    param([string]$Path)
    $text = [IO.File]::ReadAllText($Path, $script:Utf8)
    $selector = $text | ConvertFrom-Json
    $depth = 0
    $keys = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::Ordinal)
    foreach ($token in [regex]::Matches($text, '"(?:\\.|[^"\\])*"|[{}\[\]]')) {
        if ($token.Value -eq '{' -or $token.Value -eq '[') { $depth++ }
        elseif ($token.Value -eq '}' -or $token.Value -eq ']') { $depth-- }
        elseif ($depth -eq 1 -and $text.Substring($token.Index + $token.Length) -match '^\s*:') {
            $name = $token.Value | ConvertFrom-Json
            if (-not $keys.Add($name)) { throw "Duplicate QQNT version selector field: $name" }
        }
    }
    if ($selector.PSObject.Properties.Name -inotcontains 'readyVersion') {
        Add-Member -InputObject $selector -NotePropertyName 'readyVersion' -NotePropertyValue ''
    }
    foreach ($name in @('curVersion', 'baseVersion', 'buildId', 'readyVersion')) {
        if ($selector.PSObject.Properties.Name -cnotcontains $name -or $selector.$name -isnot [string]) { throw "QQNT version selector requires the exact string field '$name'." }
    }
    if ($selector.PSObject.Properties.Name -icontains 'readyInstaller') {
        if ($selector.PSObject.Properties.Name -cnotcontains 'readyInstaller' -or $selector.readyInstaller -isnot [string] -or $selector.readyInstaller -cne '') { throw 'QQNT has an invalid or pending version update installer.' }
    }
    return $selector
}

function Assert-QQProfile {
    param([string]$QQRoot, $Profile, [switch]$OriginalPackage)
    Assert-NoReparsePoint $QQRoot
    $qqntPath = $QQRoot + '\versions\' + $Profile.qqVersion + '\QQNT.dll'
    if ($qqntPath.Length -ge 260) { throw 'The QQNT.dll path must contain fewer than 260 UTF-16 code units for the native payload.' }
    $selector = Read-QQSelector (Join-SafePath $QQRoot 'versions/config.json')
    if ($selector.curVersion -cne $Profile.qqVersion) { throw 'QQNT version selector curVersion does not match the pinned profile.' }
    $matchingBases = @($Profile.selectorBases | Where-Object { $_.baseVersion -ceq $selector.baseVersion -and $_.buildId -ceq $selector.buildId })
    if ($matchingBases.Count -eq 0) { throw 'QQNT version selector baseVersion/buildId pair does not match the pinned profile.' }
    if ($selector.readyVersion -and $selector.readyVersion -cne $Profile.qqVersion) { throw 'QQNT has a pending version update. This bundle cannot launch or modify it.' }
    foreach ($entry in $Profile.files) { Assert-FileFingerprint $QQRoot $entry }
    $packagePath = Join-SafePath $QQRoot $Profile.originalPackage.path
    $package = Read-JsonFile $packagePath
    if ($package.version -cne $Profile.qqVersion -or [string]$package.buildVersion -cne [string]$Profile.buildVersion -or $package.platform -cne $Profile.platform -or $package.eleArch -cne $Profile.architecture) { throw 'QQNT package metadata does not match the pinned profile.' }
    if ($OriginalPackage) {
        Assert-FileFingerprint $QQRoot $Profile.originalPackage
        if ($package.main -cne $Profile.originalPackage.main) { throw 'QQNT package entry point is already modified.' }
    }
    return $packagePath
}

function ConvertTo-JavaScriptString {
    param([string]$Value)
    $json = New-Object Text.StringBuilder
    [void]$json.Append('"')
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '"') { [void]$json.Append('\"') }
        elseif ($character -eq '\') { [void]$json.Append('\\') }
        elseif ([int]$character -lt 32 -or [int]$character -eq 0x2028 -or [int]$character -eq 0x2029) { [void]$json.Append(('\u{0:x4}' -f [int]$character)) }
        else { [void]$json.Append($character) }
    }
    [void]$json.Append('"')
    return $json.ToString()
}

function Get-BootstrapText {
    param([string]$InstallDirectory, [string]$ProfileDirectory)
    $loader = ConvertTo-JavaScriptString (Join-Path $InstallDirectory 'loader')
    $profile = ConvertTo-JavaScriptString $ProfileDirectory
    return (@("'use strict';", "process.env.PHANTOMLOADERQQNT_PROFILE = $profile;", "require($loader);", '') -join [Environment]::NewLine)
}

function Get-ModifiedPackageBytes {
    param([byte[]]$Original)
    $text = $script:Utf8.GetString($Original)
    $main = New-Object Text.RegularExpressions.Regex('("main"\s*:\s*)"(?:\\.|[^"\\])*"')
    return ,$script:Utf8.GetBytes($main.Replace($text, ('$1"' + $script:BootstrapMain + '"'), 1))
}

function Get-BytesHash {
    param([byte[]]$Bytes)
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Assert-InstallState {
    param([string]$InstallDirectory, [string]$QQRoot)
    $bundle = Get-PlatformBundle $InstallDirectory -Installed
    $state = Read-JsonFile (Join-SafePath $bundle.Root 'state.json')
    if ($state.schemaVersion -ne 1 -or $state.product -cne 'PhantomLoaderQQNT' -or $state.installDirectory -ine $bundle.Root -or ($QQRoot -and $state.qqRoot -ine (Get-AbsolutePath $QQRoot))) { throw 'Installation receipt does not match this installation.' }
    $locations = Get-PlatformLocations $state.qqRoot $state.installDirectory $state.profileDirectory $state.desktopDirectory
    $packagePath = Assert-QQProfile $locations.QQRoot $bundle.Profile
    $backupPath = Join-SafePath $bundle.Root 'original-package.bin'
    $backup = Get-FileFingerprint $backupPath 'original-package.bin'
    if ($backup.bytes -ne $bundle.Profile.originalPackage.bytes -or $backup.sha256 -cne $bundle.Profile.originalPackage.sha256 -or $state.originalPackageSha256 -cne $backup.sha256) { throw 'Original package backup does not match the pinned original.' }
    $originalBytes = [IO.File]::ReadAllBytes($backupPath)
    $modifiedHash = Get-BytesHash (Get-ModifiedPackageBytes $originalBytes)
    $actual = Get-FileFingerprint $packagePath $bundle.Profile.originalPackage.path
    if ($actual.sha256 -cne $modifiedHash -or $state.modifiedPackageSha256 -cne $modifiedHash) { throw 'QQNT package changed after installation; refusing to overwrite or launch it.' }
    $bootstrapRelative = 'versions/' + $bundle.Profile.qqVersion + '/resources/app/app_launcher/PhantomLoaderQQNT.js'
    $bootstrap = Join-SafePath $locations.QQRoot $bootstrapRelative
    if ($state.ownsBootstrapDirectory -isnot [bool]) { throw 'The bootstrap directory ownership receipt is invalid.' }
    $bootstrapHash = Get-BytesHash $script:Utf8.GetBytes((Get-BootstrapText $bundle.Root $locations.ProfileDirectory))
    if ($state.bootstrapSha256 -cne $bootstrapHash -or (Get-FileFingerprint $bootstrap $bootstrapRelative).sha256 -cne $bootstrapHash) { throw 'The owned bootstrap has changed.' }
    $owned = @($bundle.Manifest.files) + @((Get-FileFingerprint (Join-SafePath $bundle.Root 'bundle.json') 'bundle.json'), $backup)
    Assert-ExactFileSet @($state.ownedFiles | ForEach-Object { $_.path }) @($owned | ForEach-Object { $_.path }) 'Receipt'
    foreach ($entry in $owned) {
        $recorded = @($state.ownedFiles | Where-Object { $_.path -ceq $entry.path })[0]
        if ($recorded.bytes -ne $entry.bytes -or $recorded.sha256 -cne $entry.sha256) { throw "Owned asset receipt differs: $($entry.path)" }
    }
    $shortcutPath = Join-SafePath $locations.DesktopDirectory 'QQNT (PhantomLoader).lnk'
    if ($state.shortcut.path -ine $shortcutPath) { throw 'Shortcut receipt has an unexpected path.' }
    if (Test-Path -LiteralPath $shortcutPath) {
        $shortcut = Get-FileFingerprint $shortcutPath 'QQNT (PhantomLoader).lnk'
        if ($shortcut.sha256 -cne $state.shortcut.sha256 -or $shortcut.bytes -ne $state.shortcut.bytes) { throw 'The installed shortcut has changed.' }
    }
    return [pscustomobject]@{ Bundle = $bundle; State = $state; Locations = $locations; PackagePath = $packagePath; BootstrapPath = $bootstrap; ShortcutPath = $shortcutPath; BackupPath = $backupPath }
}

function New-PlatformShortcut {
    param([string]$Path, [string]$InstallDirectory, [string]$QQRoot)
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($Path)
        $shortcut.TargetPath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $shortcut.Arguments = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "' + (Join-Path $InstallDirectory 'Launch.ps1') + '" -ShowErrorDialog'
        $shortcut.WorkingDirectory = $QQRoot
        $shortcut.IconLocation = (Join-Path $QQRoot 'QQ.exe') + ',0'
        $shortcut.Description = 'Launch QQNT with PhantomLoaderQQNT'
        $shortcut.Save()
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) }
}

function Assert-BundleOutsideDestination {
    param([string]$BundleRoot, $Locations)
    foreach ($path in @($Locations.InstallDirectory, $Locations.ProfileDirectory)) {
        if ((Test-PathWithin $BundleRoot $path) -or (Test-PathWithin $path $BundleRoot)) { throw 'Bundle, installation and profile directories must be separate.' }
    }
}

function Invoke-PlatformInstall {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string]$BundleDirectory, [string]$QQRoot, [string]$InstallDirectory, [string]$ProfileDirectory, [string]$DesktopDirectory, [switch]$CheckOnly)
    $bundle = Get-PlatformBundle $BundleDirectory -Installed:(Test-Path -LiteralPath (Join-Path $BundleDirectory 'state.json'))
    $locations = Get-PlatformLocations $QQRoot $InstallDirectory $ProfileDirectory $DesktopDirectory
    Assert-BundleOutsideDestination $bundle.Root $locations
    Assert-QQStopped
    if (Test-Path -LiteralPath $locations.InstallDirectory) {
        $installed = Assert-InstallState $locations.InstallDirectory $locations.QQRoot
        if ($installed.Locations.ProfileDirectory -ine $locations.ProfileDirectory -or $installed.Locations.DesktopDirectory -ine $locations.DesktopDirectory -or (Get-FileFingerprint (Join-Path $bundle.Root 'bundle.json') 'bundle.json').sha256 -cne (Get-FileFingerprint (Join-Path $installed.Bundle.Root 'bundle.json') 'bundle.json').sha256) { throw 'An existing installation has different paths or bundle contents.' }
        Write-Output 'The matching platform installation is already present and verified.'
        return
    }
    $packagePath = Assert-QQProfile $locations.QQRoot $bundle.Profile -OriginalPackage
    $appRoot = Split-Path -Parent $packagePath
    $bootstrap = Join-SafePath $appRoot 'app_launcher/PhantomLoaderQQNT.js'
    $shortcut = Join-SafePath $locations.DesktopDirectory 'QQNT (PhantomLoader).lnk'
    foreach ($path in @($bootstrap, $shortcut)) { if (Test-Path -LiteralPath $path) { throw "An unrelated file already occupies the destination: $path" } }
    $bootstrapDirectory = Split-Path -Parent $bootstrap
    $ownsBootstrapDirectory = -not (Test-Path -LiteralPath $bootstrapDirectory)
    if (-not $ownsBootstrapDirectory -and -not (Test-Path -LiteralPath $bootstrapDirectory -PathType Container)) { throw "The bootstrap directory path is occupied by a file: $bootstrapDirectory" }
    if (-not (Test-Path -LiteralPath $locations.DesktopDirectory -PathType Container)) { throw 'The desktop directory must already exist.' }
    if ($CheckOnly) { Write-Output 'Installation preflight passed; no files changed.'; return }
    if (-not $PSCmdlet.ShouldProcess($locations.QQRoot, 'Install PhantomLoaderQQNT platform and desktop shortcut')) { return }
    $parent = Split-Path -Parent $locations.InstallDirectory
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw 'The installation parent directory must already exist.' }
    $stage = Join-Path $parent ('.PhantomLoaderQQNT-stage-' + [Guid]::NewGuid().ToString('N'))
    $packageTemp = Join-Path $appRoot ('.PhantomLoaderQQNT-package-' + [Guid]::NewGuid().ToString('N') + '.tmp')
    $shortcutTemp = Join-Path $locations.DesktopDirectory ('.PhantomLoaderQQNT-' + [Guid]::NewGuid().ToString('N') + '.lnk')
    $bootstrapStage = Join-Path $appRoot ('.PhantomLoaderQQNT-bootstrap-' + [Guid]::NewGuid().ToString('N'))
    $createdInstall = $false
    $createdBootstrap = $false
    $createdBootstrapDirectory = $false
    $createdShortcut = $false
    try {
        [void][IO.Directory]::CreateDirectory($stage)
        foreach ($entry in $bundle.Manifest.files) {
            $destination = Join-SafePath $stage $entry.path
            [void][IO.Directory]::CreateDirectory((Split-Path -Parent $destination))
            [IO.File]::Copy((Join-SafePath $bundle.Root $entry.path), $destination, $false)
        }
        [IO.File]::Copy((Join-Path $bundle.Root 'bundle.json'), (Join-Path $stage 'bundle.json'), $false)
        [void](Get-PlatformBundle $stage)
        $original = [IO.File]::ReadAllBytes($packagePath)
        [IO.File]::WriteAllBytes((Join-Path $stage 'original-package.bin'), $original)
        $bootstrapBytes = $script:Utf8.GetBytes((Get-BootstrapText $locations.InstallDirectory $locations.ProfileDirectory))
        $modified = Get-ModifiedPackageBytes $original
        [IO.File]::WriteAllBytes($packageTemp, $modified)
        New-PlatformShortcut $shortcutTemp $locations.InstallDirectory $locations.QQRoot
        $shortcutEntry = Get-FileFingerprint $shortcutTemp 'QQNT (PhantomLoader).lnk'
        $owned = @($bundle.Manifest.files) + @((Get-FileFingerprint (Join-Path $stage 'bundle.json') 'bundle.json'), (Get-FileFingerprint (Join-Path $stage 'original-package.bin') 'original-package.bin'))
        $state = [ordered]@{
            schemaVersion = 1; product = 'PhantomLoaderQQNT'; qqRoot = $locations.QQRoot; installDirectory = $locations.InstallDirectory
            profileDirectory = $locations.ProfileDirectory; desktopDirectory = $locations.DesktopDirectory
            originalPackageSha256 = $bundle.Profile.originalPackage.sha256; modifiedPackageSha256 = (Get-BytesHash $modified)
            bootstrapSha256 = (Get-BytesHash $bootstrapBytes); ownsBootstrapDirectory = $ownsBootstrapDirectory; ownedFiles = $owned
            shortcut = @{ path = $shortcut; bytes = $shortcutEntry.bytes; sha256 = $shortcutEntry.sha256 }
        }
        Write-JsonFile (Join-Path $stage 'state.json') $state
        if ($ownsBootstrapDirectory) {
            [void][IO.Directory]::CreateDirectory($bootstrapStage)
            [IO.File]::WriteAllBytes((Join-Path $bootstrapStage 'PhantomLoaderQQNT.js'), $bootstrapBytes)
        }
        Assert-QQStopped
        [void](Assert-QQProfile $locations.QQRoot $bundle.Profile -OriginalPackage)
        [IO.Directory]::Move($stage, $locations.InstallDirectory)
        $createdInstall = $true
        if ($ownsBootstrapDirectory) {
            [IO.Directory]::Move($bootstrapStage, $bootstrapDirectory)
            $createdBootstrapDirectory = $true
            $createdBootstrap = $true
        } else {
            $stream = New-Object IO.FileStream($bootstrap, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write)
            $createdBootstrap = $true
            try { $stream.Write($bootstrapBytes, 0, $bootstrapBytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
        }
        [IO.File]::Move($shortcutTemp, $shortcut)
        $createdShortcut = $true
        [IO.File]::Replace($packageTemp, $packagePath, [NullString]::Value)
    } catch {
        if ($createdShortcut) { [IO.File]::Delete($shortcut) }
        if ($createdBootstrap) { [IO.File]::Delete($bootstrap) }
        if ($createdBootstrapDirectory -and @(Get-ChildItem -LiteralPath $bootstrapDirectory -Force).Count -eq 0) { [IO.Directory]::Delete($bootstrapDirectory) }
        if ($createdInstall) { Remove-Item -LiteralPath $locations.InstallDirectory -Recurse -Force -Confirm:$false }
        throw "Installation failed and its new assets were rolled back. If Windows denied access under Program Files, rerun PowerShell as administrator. $($_.Exception.Message)"
    } finally {
        foreach ($path in @($packageTemp, $shortcutTemp)) { if (Test-Path -LiteralPath $path) { [IO.File]::Delete($path) } }
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -Confirm:$false }
        if (Test-Path -LiteralPath $bootstrapStage) { Remove-Item -LiteralPath $bootstrapStage -Recurse -Force -Confirm:$false }
    }
    Write-Output "Installed successfully. Launch with '$shortcut'. Profile: $($locations.ProfileDirectory)"
}

function Invoke-PlatformInjector {
    param([string]$Executable, [string[]]$Arguments)
    $ErrorActionPreference = 'Continue'
    $output = & $Executable @Arguments 2>&1
    $exitCode = [uint32]([int64]$LASTEXITCODE -band 4294967295L)
    $codes = $script:NativeCapture.exitCodes
    if (@(0, [uint32]$codes.captured, [uint32]$codes.dumpFailed, [uint32]$codes.armFailed) -notcontains $exitCode) { throw ("The QQNT launcher or monitored process failed with exit code 0x{0:X8}: {1}" -f $exitCode, ($output | Out-String)) }
    $output
}

function Invoke-PlatformLaunch {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([Parameter(Mandatory = $true)][string]$InstallDirectory, [switch]$CheckOnly)
    $installed = Assert-InstallState $InstallDirectory
    Assert-QQStopped
    if ($CheckOnly) { Write-Output 'Launch verification passed; no process started.'; return }
    if (-not $PSCmdlet.ShouldProcess($installed.Locations.QQRoot, 'Launch verified QQNT through the platform injector')) { return }
    $captures = Join-SafePath $installed.Locations.ProfileDirectory 'data/dlvm_capture/captures'
    [void][IO.Directory]::CreateDirectory($captures)
    $env:PHANTOMLOADERQQNT_PROFILE = $installed.Locations.ProfileDirectory
    Invoke-PlatformInjector (Join-SafePath $installed.Bundle.Root 'payload/phantomloader.exe') @('-exe', (Join-SafePath $installed.Locations.QQRoot 'QQ.exe'), '-payload', (Join-SafePath $installed.Bundle.Root 'payload/phantomloader_payload.dll'), '-reporter', (Join-SafePath $installed.Bundle.Root 'Report-Capture.ps1'), '-capture-directory', $captures)
}

function Invoke-PlatformUninstall {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param([string]$QQRoot, [string]$InstallDirectory, [switch]$CheckOnly)
    $root = Resolve-PlatformQQRoot $QQRoot
    if (-not $InstallDirectory) { $InstallDirectory = Join-Path $root 'PhantomLoaderQQNT' }
    $installed = Assert-InstallState $InstallDirectory $root
    Assert-QQStopped
    if ($CheckOnly) { Write-Output 'Uninstallation preflight passed; all profile plugins and data will be retained.'; return }
    if (-not $PSCmdlet.ShouldProcess($root, 'Restore the original QQNT package and remove owned platform files; retain the entire profile')) { return }
    $token = [Guid]::NewGuid().ToString('N')
    $temporary = Join-Path (Split-Path -Parent $installed.PackagePath) ('.PhantomLoaderQQNT-restore-' + $token + '.tmp')
    $retiredInstall = Join-Path (Split-Path -Parent $installed.Bundle.Root) ('.PhantomLoaderQQNT-remove-' + $token)
    $retiredBootstrap = Join-Path (Split-Path -Parent $installed.BootstrapPath) ('.PhantomLoaderQQNT-remove-' + $token + '.js')
    $retiredShortcut = Join-Path (Split-Path -Parent $installed.ShortcutPath) ('.PhantomLoaderQQNT-remove-' + $token + '.lnk')
    $movedInstall = $false; $movedBootstrap = $false; $movedShortcut = $false
    try {
        [IO.File]::Copy($installed.BackupPath, $temporary, $false)
        Assert-QQStopped
        [void](Assert-InstallState $InstallDirectory $root)
        [IO.File]::Move($installed.BootstrapPath, $retiredBootstrap)
        $movedBootstrap = $true
        if (Test-Path -LiteralPath $installed.ShortcutPath) {
            [IO.File]::Move($installed.ShortcutPath, $retiredShortcut)
            $movedShortcut = $true
        }
        [IO.Directory]::Move($installed.Bundle.Root, $retiredInstall)
        $movedInstall = $true
        [IO.File]::Replace($temporary, $installed.PackagePath, [NullString]::Value)
    } catch {
        if ($movedInstall) { [IO.Directory]::Move($retiredInstall, $installed.Bundle.Root) }
        if ($movedShortcut) { [IO.File]::Move($retiredShortcut, $installed.ShortcutPath) }
        if ($movedBootstrap) { [IO.File]::Move($retiredBootstrap, $installed.BootstrapPath) }
        throw "Uninstallation stopped and its moves were rolled back. The profile was retained. If Windows denied access under Program Files, rerun PowerShell as administrator. $($_.Exception.Message)"
    } finally { if (Test-Path -LiteralPath $temporary) { [IO.File]::Delete($temporary) } }
    try {
        [IO.File]::Delete($retiredBootstrap)
        $bootstrapDirectory = Split-Path -Parent $installed.BootstrapPath
        if ($installed.State.ownsBootstrapDirectory -and @(Get-ChildItem -LiteralPath $bootstrapDirectory -Force).Count -eq 0) { [IO.Directory]::Delete($bootstrapDirectory) }
        if ($movedShortcut) { [IO.File]::Delete($retiredShortcut) }
        Remove-Item -LiteralPath $retiredInstall -Recurse -Force -Confirm:$false
    } catch {
        throw "The original package was restored, but Windows prevented removal of retired platform assets: '$retiredInstall', '$retiredBootstrap', '$retiredShortcut'. The profile was retained. $($_.Exception.Message)"
    }
    Write-Output "Original package bytes restored. Profile retained: $($installed.Locations.ProfileDirectory)"
}

Export-ModuleMember -Function Invoke-PlatformInstall, Invoke-PlatformLaunch, Invoke-PlatformUninstall
