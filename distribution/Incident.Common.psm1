#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$script:Utf8 = New-Object System.Text.UTF8Encoding($false, $true)
$script:IssueDestination = 'https://github.com/mlbv/PhantomLoaderQQNT/issues/new'
Add-Type -AssemblyName System.IO.Compression

function Assert-IncidentDirectory {
    param([string]$Path)
    if (-not [IO.Path]::IsPathRooted($Path) -or [IO.Path]::GetPathRoot($Path) -notmatch '[\\/]$') {
        throw 'The capture directory must be an absolute Windows path.'
    }
    $full = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $cursor = $full
    while ($cursor) {
        $item = Get-Item -LiteralPath $cursor -Force -ErrorAction Stop
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Capture directories must be ordinary directories, without reparse points.'
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
    return $full
}

function Open-IncidentFile {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Capture evidence must be ordinary files, without reparse points.'
    }
    return [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
}

function Get-IncidentStreamHash {
    param([IO.Stream]$Stream)
    $hash = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($hash.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant() }
    finally { $hash.Dispose() }
}

function Test-IncidentInteger {
    param($Value, [uint64]$Maximum)
    return ($Value -is [int] -or $Value -is [long] -or $Value -is [uint32] -or $Value -is [uint64]) -and $Value -ge 0 -and $Value -le $Maximum
}

function Get-IncidentCandidate {
    param([string]$Directory, [uint32]$ProcessId, [uint64]$Started, [uint64]$Ended)
    $stems = @{}
    foreach ($path in [IO.Directory]::EnumerateFiles($Directory)) {
        $name = [IO.Path]::GetFileName($path)
        if ($name -cnotmatch '^dlvm-([1-9][0-9]{0,18})-([1-9][0-9]{0,9})-([1-9][0-9]{0,9})\.(bin|json)$') { continue }
        [uint64]$ticks = 0
        [uint32]$owner = 0
        [uint32]$thread = 0
        if (-not [uint64]::TryParse($Matches[1], [ref]$ticks) -or
            -not [uint32]::TryParse($Matches[2], [ref]$owner) -or
            -not [uint32]::TryParse($Matches[3], [ref]$thread)) { continue }
        if ($owner -ne $ProcessId -or $ticks -lt $Started -or $ticks -gt $Ended) { continue }
        $stem = [IO.Path]::GetFileNameWithoutExtension($name)
        $stems[$stem] = $ticks
    }
    if ($stems.Count -ne 1) { throw 'Expected exactly one capture pair for this process lifetime.' }
    foreach ($stem in $stems.Keys) {
        return [pscustomobject]@{ Stem = $stem; Filetime = $stems[$stem] }
    }
}

function Read-IncidentMetadata {
    param([IO.Stream]$Stream, [string]$PayloadName, [uint64]$Filetime)
    if ($Stream.Length -eq 0 -or $Stream.Length -gt 16384) { throw 'Capture metadata has an invalid size.' }
    $reader = New-Object IO.StreamReader($Stream, $script:Utf8, $true, 1024, $true)
    try { $metadata = $reader.ReadToEnd() | ConvertFrom-Json -ErrorAction Stop }
    finally { $reader.Dispose() }
    if ($null -eq $metadata -or $metadata -isnot [pscustomobject]) { throw 'Capture metadata must be a JSON object.' }
    $fields = @('build', 'wrapper_sha256', 'stage', 'task_type', 'received_bytes', 'captured_filetime', 'payload_file', 'payload_write_error')
    $properties = @($metadata.PSObject.Properties)
    if ($properties.Count -ne $fields.Count) { throw 'Capture metadata has unexpected fields.' }
    foreach ($property in $properties) {
        if ($fields -cnotcontains $property.Name) { throw 'Capture metadata has unexpected fields.' }
    }
    if ($metadata.build -isnot [string] -or $metadata.build -cnotmatch '^[A-Za-z0-9][A-Za-z0-9.+_-]{0,63}$' -or
        $metadata.wrapper_sha256 -isnot [string] -or $metadata.wrapper_sha256 -cnotmatch '^[a-f0-9]{64}$' -or
        $metadata.stage -isnot [string] -or @('receive', 'receive-grow', 'execute') -cnotcontains $metadata.stage -or
        -not (Test-IncidentInteger $metadata.task_type ([uint32]::MaxValue)) -or
        -not (Test-IncidentInteger $metadata.received_bytes ([long]::MaxValue)) -or
        -not (Test-IncidentInteger $metadata.payload_write_error ([uint32]::MaxValue)) -or
        $metadata.captured_filetime -isnot [string] -or $metadata.captured_filetime -cne $Filetime.ToString([Globalization.CultureInfo]::InvariantCulture) -or
        $metadata.payload_file -isnot [string] -or $metadata.payload_file -cne $PayloadName) {
        throw 'Capture metadata fields do not match the expected types or capture filename.'
    }
    if (($metadata.stage -ceq 'execute' -and $metadata.task_type -ne 0) -or
        ($metadata.stage -cne 'execute' -and $metadata.task_type -notin @(15, 16))) { throw 'Capture stage and task type disagree.' }
    if ($metadata.payload_write_error -ne 0) { throw 'The native writer reported an incomplete payload.' }
    return $metadata
}

function Get-IncidentMetadataBytes {
    param($Metadata)
    $json = '{"build":"' + $Metadata.build + '","wrapper_sha256":"' + $Metadata.wrapper_sha256 +
        '","stage":"' + $Metadata.stage + '","task_type":' + $Metadata.task_type.ToString([Globalization.CultureInfo]::InvariantCulture) +
        ',"received_bytes":' + $Metadata.received_bytes.ToString([Globalization.CultureInfo]::InvariantCulture) +
        ',"captured_filetime":"' + $Metadata.captured_filetime + '","payload_file":"' + $Metadata.payload_file + '","payload_write_error":0}' + "`n"
    return ,$script:Utf8.GetBytes($json)
}

function Get-IncidentDetails {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)]$Report)
    $lines = New-Object 'Collections.Generic.List[string]'
    $lines.Add('PhantomLoaderQQNT DLVM incident')
    $lines.Add('')
    $lines.Add($Report.Summary)
    $lines.Add(('Process ID: {0}; exit code: 0x{1:X8}' -f $Report.ProcessId, $Report.ExitCode))
    if ($Report.PlatformVersion) {
        $lines.Add('PhantomLoaderQQNT release: ' + $Report.PlatformVersion)
        $lines.Add('Distribution profile SHA-256: ' + $Report.ProfileSha256)
    }
    $lines.Add('Evidence: ' + $Report.EvidenceStatus)
    if ($Report.EvidenceError) { $lines.Add('Details: ' + $Report.EvidenceError) }
    if ($Report.Metadata) {
        $lines.Add('QQNT build: ' + $Report.Metadata.build)
        $lines.Add('Wrapper SHA-256: ' + $Report.Metadata.wrapper_sha256)
        $lines.Add('Capture stage: ' + $Report.Metadata.stage + '; task type: ' + $Report.Metadata.task_type)
        $lines.Add('Capture FILETIME: ' + $Report.Metadata.captured_filetime)
        $lines.Add(('Sample: {0} ({1} bytes)' -f $Report.PayloadName, $Report.PayloadBytes))
        $lines.Add('Sample SHA-256: ' + $Report.PayloadSha256)
        $lines.Add(('Metadata: {0} ({1} bytes, validated JSON)' -f $Report.MetadataName, $Report.MetadataBytes))
        $lines.Add('Metadata SHA-256: ' + $Report.MetadataSha256)
    }
    return $lines.ToArray() -join "`n"
}

function Add-IncidentZipEntry {
    param([IO.Compression.ZipArchive]$Archive, [string]$Name, [IO.Stream]$Source)
    $entry = $Archive.CreateEntry($Name, [IO.Compression.CompressionLevel]::NoCompression)
    $entry.LastWriteTime = New-Object DateTimeOffset(1980, 1, 1, 0, 0, 0, ([TimeSpan]::Zero))
    $target = $entry.Open()
    try { $Source.Position = 0; $Source.CopyTo($target) }
    finally { $target.Dispose() }
}

function Assert-IncidentZipContent {
    param([IO.Stream]$Stream, [hashtable]$Expected)
    $archive = New-Object IO.Compression.ZipArchive($Stream, [IO.Compression.ZipArchiveMode]::Read, $true)
    try {
        if ($archive.Entries.Count -ne $Expected.Count) { throw 'The ZIP has an unexpected file inventory.' }
        $seen = @{}
        foreach ($entry in $archive.Entries) {
            if (-not $Expected.ContainsKey($entry.FullName) -or $seen.ContainsKey($entry.FullName)) { throw 'The ZIP has unexpected or repeated entries.' }
            $seen[$entry.FullName] = $true
            $expectedFile = $Expected[$entry.FullName]
            if ($entry.Length -ne $expectedFile.Bytes) { throw 'The ZIP entry size differs from the captured file.' }
            $source = $entry.Open()
            try { $hash = Get-IncidentStreamHash $source }
            finally { $source.Dispose() }
            if ($hash -cne $expectedFile.Hash) { throw 'The ZIP entry hash differs from the captured file.' }
        }
    } finally { $archive.Dispose() }
}

function Assert-IncidentArchive {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)]$Report)
    if (-not $Report.ArchivePath) { throw 'No verified report ZIP is available.' }
    $stream = Open-IncidentFile $Report.ArchivePath
    try {
        if ($stream.Length -ne $Report.ArchiveBytes -or (Get-IncidentStreamHash $stream) -cne $Report.ArchiveSha256) {
            throw 'The report ZIP has changed. Close this window and preserve the original captures.'
        }
    } finally { $stream.Dispose() }
}

function New-IncidentReport {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$CaptureDirectory,
        [Parameter(Mandatory = $true)][uint32]$ProcessId,
        [Parameter(Mandatory = $true)][uint32]$ExitCode,
        [Parameter(Mandatory = $true)][uint64]$ProcessStartedFiletime,
        [uint64]$ProcessExitedFiletime = [uint64][DateTime]::UtcNow.ToFileTimeUtc()
    )
    $report = [pscustomobject]@{
        ProcessId = $ProcessId; ExitCode = $ExitCode; IssueDestination = $script:IssueDestination
        PlatformVersion = ''; ProfileSha256 = ''
        Summary = ''; EvidenceStatus = ''; EvidenceError = ''; Metadata = $null
        PayloadName = ''; PayloadBytes = [long]0; PayloadSha256 = ''
        MetadataName = ''; MetadataBytes = [long]0; MetadataSha256 = ''
        ArchiveName = ''; ArchivePath = ''; ArchiveBytes = [long]0; ArchiveSha256 = ''
    }
    $profileStream = Open-IncidentFile (Join-Path $PSScriptRoot 'profile.json')
    try {
        if ($profileStream.Length -eq 0 -or $profileStream.Length -gt 65536) { throw 'The distribution profile has an invalid size.' }
        $profileReader = New-Object IO.StreamReader($profileStream, $script:Utf8, $true, 1024, $true)
        try { $profile = $profileReader.ReadToEnd() | ConvertFrom-Json -ErrorAction Stop }
        finally { $profileReader.Dispose() }
        if ($profile.schemaVersion -ne 1 -or $profile.platformVersion -isnot [string] -or $profile.platformVersion -cnotmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
            throw 'The distribution profile release is invalid.'
        }
        $report.PlatformVersion = $profile.platformVersion
        $profileStream.Position = 0
        $report.ProfileSha256 = Get-IncidentStreamHash $profileStream
    } finally { $profileStream.Dispose() }
    $exitCodes = $profile.native.capture.exitCodes
    if ($ExitCode -notin @([uint32]$exitCodes.captured, [uint32]$exitCodes.dumpFailed, [uint32]$exitCodes.armFailed)) { throw 'This exit code does not describe a capture incident.' }
    if ($ExitCode -eq $exitCodes.dumpFailed) {
        $report.Summary = 'QQ terminated after a DLVM capture write failure.'
        $report.EvidenceStatus = 'No verified ZIP is available; the capture may be incomplete or missing.'
        return $report
    }
    if ($ExitCode -eq $exitCodes.armFailed) {
        $report.Summary = 'QQ terminated because the native DLVM capture guard could not be armed.'
        $report.EvidenceStatus = 'No capture is confirmed. No ZIP was created.'
        return $report
    }
    $report.Summary = 'QQ terminated after the native DLVM capture signal.'
    $report.EvidenceStatus = 'The capture could not be validated or packaged. No ZIP is ready to attach.'
    $step = 'The process lifetime or capture directory is invalid.'
    $payload = $null; $metadataStream = $null; $canonicalStream = $null; $textStream = $null; $temporary = $null
    try {
        if ($ProcessId -eq 0 -or $ProcessStartedFiletime -eq 0 -or $ProcessStartedFiletime -gt $ProcessExitedFiletime -or $ProcessExitedFiletime -gt [long]::MaxValue) {
            throw 'Invalid process lifetime.'
        }
        [void][DateTime]::FromFileTimeUtc([long]$ProcessStartedFiletime)
        [void][DateTime]::FromFileTimeUtc([long]$ProcessExitedFiletime)
        $directory = Assert-IncidentDirectory $CaptureDirectory
        $step = 'Expected one matching sample/metadata pair within this process lifetime; no older or ambiguous pair can be attached.'
        $candidate = Get-IncidentCandidate $directory $ProcessId $ProcessStartedFiletime $ProcessExitedFiletime
        $payloadName = $candidate.Stem + '.bin'
        $metadataName = $candidate.Stem + '.json'
        $step = 'The matching sample or metadata is missing, unreadable, or not an ordinary file.'
        $payload = Open-IncidentFile ([IO.Path]::Combine($directory, $payloadName))
        $metadataStream = Open-IncidentFile ([IO.Path]::Combine($directory, $metadataName))
        $step = 'The metadata is malformed, contains unexpected fields, or disagrees with the capture name, size, or write result.'
        $metadata = Read-IncidentMetadata $metadataStream $payloadName $candidate.Filetime
        if ($payload.Length -ne $metadata.received_bytes) { throw 'The payload length differs from received_bytes.' }
        $canonical = Get-IncidentMetadataBytes $metadata
        $canonicalStream = New-Object IO.MemoryStream(,$canonical)
        $report.Metadata = $metadata
        $report.PayloadName = $payloadName; $report.PayloadBytes = $payload.Length
        $report.PayloadSha256 = Get-IncidentStreamHash $payload
        $report.MetadataName = $metadataName; $report.MetadataBytes = $canonical.Length
        $report.MetadataSha256 = Get-IncidentStreamHash $canonicalStream
        $report.Summary = 'A DLVM payload was saved locally and QQ terminated.'
        $report.EvidenceStatus = 'The sample and metadata were validated. Raw sample bytes were not executed or decoded.'
        $reportText = (Get-IncidentDetails $report) + "`n`nArchive contents: sample .bin, validated metadata .json, and this report.txt only.`nNo full dumps, logs, account data, or local profile paths are included by the reporter.`nThe raw sample may itself contain sensitive data. Review it before public submission.`n"
        $textStream = New-Object IO.MemoryStream(,$script:Utf8.GetBytes($reportText))
        $expected = @{}
        $expected[$payloadName] = @{ Bytes = $payload.Length; Hash = $report.PayloadSha256 }
        $expected[$metadataName] = @{ Bytes = $canonical.Length; Hash = $report.MetadataSha256 }
        $expected['report.txt'] = @{ Bytes = $textStream.Length; Hash = (Get-IncidentStreamHash $textStream) }
        $step = 'The validated capture could not be packaged into a verified local ZIP.'
        $reportsDirectory = [IO.Path]::Combine($directory, 'reports')
        [void][IO.Directory]::CreateDirectory($reportsDirectory)
        [void](Assert-IncidentDirectory $reportsDirectory)
        $temporary = [IO.Path]::Combine($reportsDirectory, ('.incident-' + [Guid]::NewGuid().ToString('N') + '.tmp'))
        $zipStream = [IO.File]::Open($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
        try {
            $zip = New-Object IO.Compression.ZipArchive($zipStream, [IO.Compression.ZipArchiveMode]::Create, $true)
            try {
                Add-IncidentZipEntry $zip $payloadName $payload
                Add-IncidentZipEntry $zip $metadataName $canonicalStream
                Add-IncidentZipEntry $zip 'report.txt' $textStream
            } finally { $zip.Dispose() }
            $zipStream.Flush()
            $zipStream.Position = 0
            Assert-IncidentZipContent $zipStream $expected
            $zipStream.Position = 0
            $zipHash = Get-IncidentStreamHash $zipStream
            $zipBytes = $zipStream.Length
        } finally { $zipStream.Dispose() }
        $archiveName = 'dlvm-report-' + $zipHash + '.zip'
        $archivePath = [IO.Path]::Combine($reportsDirectory, $archiveName)
        if ([IO.File]::Exists($archivePath)) {
            $existing = Open-IncidentFile $archivePath
            try {
                if ($existing.Length -ne $zipBytes -or (Get-IncidentStreamHash $existing) -cne $zipHash) { throw 'An existing report ZIP failed hash verification.' }
            } finally { $existing.Dispose() }
        } else { [IO.File]::Move($temporary, $archivePath) }
        $report.ArchiveName = $archiveName; $report.ArchivePath = $archivePath
        $report.ArchiveBytes = $zipBytes; $report.ArchiveSha256 = $zipHash
        Assert-IncidentArchive $report
        return $report
    } catch {
        $report.Summary = 'QQ terminated after the native DLVM capture signal.'
        $report.EvidenceStatus = 'The capture could not be validated or packaged. No ZIP is ready to attach.'
        $report.EvidenceError = $step
        $report.Metadata = $null
        $report.ArchiveName = ''; $report.ArchivePath = ''; $report.ArchiveBytes = 0; $report.ArchiveSha256 = ''
        return $report
    } finally {
        foreach ($stream in @($payload, $metadataStream, $canonicalStream, $textStream)) { if ($null -ne $stream) { $stream.Dispose() } }
        if ($temporary -and [IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) }
    }
}

function Get-IncidentIssueUri {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)]$Report)
    $title = 'DLVM capture incident'
    if ($Report.Metadata) { $title += ' - ' + $Report.Metadata.build + ' - ' + $Report.Metadata.stage + ' - ' + $Report.PayloadSha256.Substring(0, 12) }
    $body = (Get-IncidentDetails $Report) + "`n`n"
    if ($Report.ArchivePath) {
        $body += 'Attach ZIP: ' + $Report.ArchiveName + "`n" + 'ZIP bytes: ' + $Report.ArchiveBytes + "`n" + 'ZIP SHA-256: ' + $Report.ArchiveSha256 + "`n"
    }
    $body += "`nPlease add what you were doing when QQ closed.`n`nPublic submission: this issue and any attachment will be public. Review the raw sample for sensitive data before attaching it. Nothing has been uploaded automatically.`n"
    return $script:IssueDestination + '?title=' + [Uri]::EscapeDataString($title) + '&body=' + [Uri]::EscapeDataString($body)
}

Export-ModuleMember -Function New-IncidentReport, Get-IncidentDetails, Get-IncidentIssueUri, Assert-IncidentArchive
