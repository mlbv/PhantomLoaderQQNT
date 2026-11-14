#requires -Version 7.0
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

<#
.SYNOPSIS
Verifies a release tag and its directly referenced commit against a pinned OpenPGP key.
.PARAMETER ExpectedCommit
The full commit SHA from the tag push event (github.sha), not the annotated tag object's SHA.
.OUTPUTS
A PSCustomObject with Commit, Tag, TagObject, and PrimaryFingerprint. Commit is the direct
commit target; TagObject is the distinct annotated tag object SHA.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('\Av(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\z')]
    [string]$Tag,

    [Parameter(Mandatory)]
    [ValidatePattern('\A[0-9a-fA-F]{40}\z')]
    [string]$ExpectedCommit,

    [string]$KeyFile = (Join-Path $PSScriptRoot '../keys/release.asc'),

    [ValidatePattern('\A[0-9a-fA-F]{40}\z')]
    [string]$ExpectedPrimaryFingerprint = '55BD0E0BE592652ED242FD849ED1BD313EFD5297'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-ReleaseTool([string]$Executable, [string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.WorkingDirectory = $PWD.ProviderPath
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        [void]$process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = $stdout.GetAwaiter().GetResult()
            Stderr = $stderr.GetAwaiter().GetResult()
        }
    } finally { $process.Dispose() }
}

function Invoke-ReleaseGit([string[]]$Arguments) {
    $result = Invoke-ReleaseTool $git (@('--no-replace-objects', '-c', 'gpg.format=openpgp',
        '-c', "gpg.program=$gpg", '-c', "gpg.openpgp.program=$gpg") + $Arguments)
    if ($result.ExitCode -ne 0) {
        throw "git $($Arguments -join ' ') failed ($($result.ExitCode)): $($result.Stderr.Trim())"
    }
    return $result
}

function Assert-ReleaseSignature([string]$Command, [string]$Object) {
    $result = Invoke-ReleaseGit @($Command, '--raw', $Object)
    $status = @((($result.Stdout + "`n" + $result.Stderr) -split '\r?\n') |
        Where-Object { $_.StartsWith('[GNUPG:] ') } | ForEach-Object { $_.Substring(9) })
    $invalid = @($status | Where-Object {
        $_ -match '\A(?:BADSIG|ERRSIG|EXPSIG|EXPKEYSIG|REVKEYSIG|KEYEXPIRED|SIGEXPIRED|KEYREVOKED|NO_PUBKEY|FAILURE|ERROR|BADARMOR|NODATA)(?: |\z)'
    })
    $valid = @($status | Where-Object { $_.StartsWith('VALIDSIG ') })
    $good = @($status | Where-Object { $_.StartsWith('GOODSIG ') })
    if ($invalid.Count -ne 0 -or $valid.Count -ne 1 -or $good.Count -ne 1) {
        throw "$Command did not produce one valid, unexpired, unrevoked OpenPGP signature for $Object."
    }
    $fields = $valid[0] -split ' '
    # GnuPG appends the primary fingerprint for a signing subkey; older versions may
    # omit that final field for a signature made by the primary key itself.
    $primary = if ($fields.Count -eq 11) { $fields[10] } elseif ($fields.Count -eq 10) { $fields[1] } else { '' }
    if ($primary -ine $ExpectedPrimaryFingerprint) {
        throw "$Command signature primary fingerprint does not match $ExpectedPrimaryFingerprint."
    }
}

$git = (Get-Command git -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$gpg = (Get-Command gpg -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
$KeyFile = (Resolve-Path -LiteralPath $KeyFile).ProviderPath
$ExpectedCommit = $ExpectedCommit.ToLowerInvariant()
$ExpectedPrimaryFingerprint = $ExpectedPrimaryFingerprint.ToUpperInvariant()
$previousHome = [Environment]::GetEnvironmentVariable('GNUPGHOME', 'Process')
$temporaryHome = Join-Path ([IO.Path]::GetTempPath()) ('verify-release-' + [Guid]::NewGuid().ToString('N'))
try {
    [void][IO.Directory]::CreateDirectory($temporaryHome)
    $env:GNUPGHOME = $temporaryHome
    [IO.File]::WriteAllText((Join-Path $temporaryHome 'gpg.conf'),
        "no-auto-key-retrieve`nauto-key-locate clear`nno-auto-key-import`nno-autostart`n")
    $gpgArguments = @('--batch', '--no-tty', '--no-options', '--no-auto-key-retrieve',
        '--auto-key-locate', 'clear', '--no-auto-key-import', '--no-autostart')
    $import = Invoke-ReleaseTool $gpg ($gpgArguments + @('--import', $KeyFile))
    if ($import.ExitCode -ne 0) { throw "Public key import failed ($($import.ExitCode)): $($import.Stderr.Trim())" }
    $keys = Invoke-ReleaseTool $gpg ($gpgArguments + @('--with-colons', '--fingerprint', '--list-keys'))
    if ($keys.ExitCode -ne 0) { throw "Public key listing failed ($($keys.ExitCode)): $($keys.Stderr.Trim())" }
    $records = @($keys.Stdout -split '\r?\n')
    $primaryKeys = @($records | Where-Object { $_.StartsWith('pub:') })
    $fingerprints = @($records | Where-Object { $_.StartsWith('fpr:') })
    if ($primaryKeys.Count -ne 1 -or $fingerprints.Count -eq 0 -or
        ($fingerprints[0] -split ':')[9] -ine $ExpectedPrimaryFingerprint) {
        throw "KeyFile must contain only the public key with primary fingerprint $ExpectedPrimaryFingerprint."
    }

    $tagObject = (Invoke-ReleaseGit @('rev-parse', '--verify', "refs/tags/$Tag")).Stdout.Trim()
    $type = (Invoke-ReleaseGit @('cat-file', '-t', $tagObject)).Stdout.Trim()
    if ($type -cne 'tag') { throw "$Tag must be a signed annotated tag." }
    $tagLines = (Invoke-ReleaseGit @('cat-file', 'tag', $tagObject)).Stdout -split '\r?\n'
    if ($tagLines[0] -cnotmatch '\Aobject [0-9a-f]{40}\z' -or $tagLines[1] -cne 'type commit') {
        throw "$Tag must point directly to a commit; nested tags are not accepted."
    }
    if ($tagLines[2] -cne "tag $Tag") { throw "The embedded tag name does not match $Tag." }
    $commit = $tagLines[0].Substring(7)
    if ($commit -cne $ExpectedCommit) { throw "$Tag targets $commit, not expected commit $ExpectedCommit." }
    if ((Invoke-ReleaseGit @('cat-file', '-t', $commit)).Stdout.Trim() -cne 'commit') {
        throw "$Tag does not reference a commit object."
    }
    Assert-ReleaseSignature 'verify-tag' $tagObject
    Assert-ReleaseSignature 'verify-commit' $commit
    [pscustomobject]@{
        Commit = $commit
        Tag = $Tag
        TagObject = $tagObject
        PrimaryFingerprint = $ExpectedPrimaryFingerprint
    }
} finally {
    [Environment]::SetEnvironmentVariable('GNUPGHOME', $previousHome, 'Process')
    if ([IO.Directory]::Exists($temporaryHome)) { [IO.Directory]::Delete($temporaryHome, $true) }
}
