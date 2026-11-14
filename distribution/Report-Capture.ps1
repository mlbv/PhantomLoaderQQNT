#requires -Version 5.1
# SPDX-FileCopyrightText: 2026 movblock
# SPDX-License-Identifier: LGPL-3.0-or-later

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$CaptureDirectory,
    [Parameter(Mandatory = $true)][uint32]$ProcessId,
    [Parameter(Mandatory = $true)][uint32]$ExitCode,
    [Parameter(Mandatory = $true)][uint64]$ProcessStartedFiletime,
    [Parameter(Mandatory = $true)][uint64]$ProcessExitedFiletime
)
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
if ([Threading.Thread]::CurrentThread.GetApartmentState() -ne [Threading.ApartmentState]::STA) {
    throw 'Run Report-Capture.ps1 with PowerShell -STA.'
}
Import-Module (Join-Path $PSScriptRoot 'Incident.Common.psm1') -Force -ErrorAction Stop
[Windows.Forms.Application]::EnableVisualStyles()

function Open-ReportDestination {
    param([string]$FileName, [string]$Arguments = '')
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $FileName
    $start.Arguments = $Arguments
    $start.UseShellExecute = $true
    [void][Diagnostics.Process]::Start($start)
}

$report = New-IncidentReport -CaptureDirectory $CaptureDirectory -ProcessId $ProcessId -ExitCode $ExitCode -ProcessStartedFiletime $ProcessStartedFiletime -ProcessExitedFiletime $ProcessExitedFiletime
$details = Get-IncidentDetails $report
if ($report.ArchivePath) {
    $details += "`n`nZIP: " + $report.ArchiveName + "`nZIP bytes: " + $report.ArchiveBytes + "`nZIP SHA-256: " + $report.ArchiveSha256
}
$form = New-Object Windows.Forms.Form
$form.Text = 'PhantomLoaderQQNT - capture report'
$form.StartPosition = [Windows.Forms.FormStartPosition]::CenterScreen
$form.ClientSize = New-Object Drawing.Size(840, 640)
$form.MinimumSize = New-Object Drawing.Size(720, 590)
$form.Font = New-Object Drawing.Font('Segoe UI', 10)
$form.AutoScaleMode = [Windows.Forms.AutoScaleMode]::Dpi
$form.ShowInTaskbar = $true

$layout = New-Object Windows.Forms.TableLayoutPanel
$layout.Dock = [Windows.Forms.DockStyle]::Fill
$layout.Padding = New-Object Windows.Forms.Padding(16)
$layout.ColumnCount = 1
$layout.RowCount = 7
[void]$layout.ColumnStyles.Add((New-Object Windows.Forms.ColumnStyle([Windows.Forms.SizeType]::Percent, 100)))
foreach ($row in 0..6) {
    $style = New-Object Windows.Forms.RowStyle
    if ($row -eq 3) { $style.SizeType = [Windows.Forms.SizeType]::Percent; $style.Height = 100 }
    else { $style.SizeType = [Windows.Forms.SizeType]::AutoSize }
    [void]$layout.RowStyles.Add($style)
}
$form.Controls.Add($layout)

$summary = New-Object Windows.Forms.Label
$summary.AutoSize = $true
$summary.Dock = [Windows.Forms.DockStyle]::Fill
$summary.Font = New-Object Drawing.Font('Segoe UI', 13, [Drawing.FontStyle]::Bold)
$summary.Text = $report.Summary
$summary.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 12)
$layout.Controls.Add($summary, 0, 0)

$status = New-Object Windows.Forms.Label
$status.AutoSize = $true
$status.Dock = [Windows.Forms.DockStyle]::Fill
$status.Text = $report.EvidenceStatus + "`r`nOriginal capture files are kept in place."
$status.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 10)
$layout.Controls.Add($status, 0, 1)

$destination = New-Object Windows.Forms.Label
$destination.AutoSize = $true
$destination.Dock = [Windows.Forms.DockStyle]::Fill
$destination.Text = 'Issue destination: github.com/mlbv/PhantomLoaderQQNT'
$destination.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 8)
$layout.Controls.Add($destination, 0, 2)

$detailsBox = New-Object Windows.Forms.TextBox
$detailsBox.Multiline = $true
$detailsBox.ReadOnly = $true
$detailsBox.WordWrap = $false
$detailsBox.ScrollBars = [Windows.Forms.ScrollBars]::Both
$detailsBox.Dock = [Windows.Forms.DockStyle]::Fill
$detailsBox.Text = $details.Replace("`n", "`r`n")
$detailsBox.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 12)
$layout.Controls.Add($detailsBox, 0, 3)

$localPath = New-Object Windows.Forms.Label
$localPath.AutoSize = $true
$localPath.Dock = [Windows.Forms.DockStyle]::Fill
$localPath.Text = 'No verified ZIP is available for attachment.'
if ($report.ArchivePath) { $localPath.Text = 'Local ZIP: ' + $report.ArchivePath }
$localPath.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 12)
$layout.Controls.Add($localPath, 0, 4)

$disclosure = New-Object Windows.Forms.Label
$disclosure.AutoSize = $true
$disclosure.Dock = [Windows.Forms.DockStyle]::Fill
$disclosure.Text = 'The GitHub issue and any attachment will be public. A ZIP contains only the raw sample, validated metadata, and report.txt. The raw sample may itself contain sensitive data; review it before attaching. Nothing is uploaded automatically. GitHub requires you to attach the ZIP manually and submit the issue.'
$disclosure.Margin = New-Object Windows.Forms.Padding(0, 0, 0, 12)
$layout.Controls.Add($disclosure, 0, 5)

$buttons = New-Object Windows.Forms.FlowLayoutPanel
$buttons.AutoSize = $true
$buttons.WrapContents = $true
$buttons.Dock = [Windows.Forms.DockStyle]::Fill
$buttons.Margin = New-Object Windows.Forms.Padding(0)
$layout.Controls.Add($buttons, 0, 6)
foreach ($caption in @('Open GitHub issue', 'Select ZIP', 'Copy details', 'Close')) {
    $button = New-Object Windows.Forms.Button
    $button.AutoSize = $true
    $button.Padding = New-Object Windows.Forms.Padding(8, 4, 8, 4)
    $button.Text = $caption
    $button.Margin = New-Object Windows.Forms.Padding(0, 0, 8, 0)
    $buttons.Controls.Add($button)
}
$issueButton = $buttons.Controls[0]
$selectButton = $buttons.Controls[1]
$copyButton = $buttons.Controls[2]
$closeButton = $buttons.Controls[3]
$selectButton.Enabled = [bool]$report.ArchivePath
$issueButton.Add_Click({
    try {
        if ($report.ArchivePath) { Assert-IncidentArchive $report }
        Open-ReportDestination (Get-IncidentIssueUri $report)
    } catch {
        [void][Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, 'Cannot open report', [Windows.Forms.MessageBoxButtons]::OK, [Windows.Forms.MessageBoxIcon]::Error)
    }
})
$selectButton.Add_Click({
    try {
        Assert-IncidentArchive $report
        $explorer = [IO.Path]::Combine([Environment]::GetFolderPath([Environment+SpecialFolder]::Windows), 'explorer.exe')
        Open-ReportDestination $explorer ('/select,"' + $report.ArchivePath + '"')
    } catch {
        [void][Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, 'Cannot select ZIP', [Windows.Forms.MessageBoxButtons]::OK, [Windows.Forms.MessageBoxIcon]::Error)
    }
})
$copyButton.Add_Click({
    try { [Windows.Forms.Clipboard]::SetText($details.Replace("`n", "`r`n")) }
    catch { [void][Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, 'Cannot copy details', [Windows.Forms.MessageBoxButtons]::OK, [Windows.Forms.MessageBoxIcon]::Error) }
})
$closeButton.Add_Click({ $form.Close() })
$form.CancelButton = $closeButton
try { [void]$form.ShowDialog() }
finally { $form.Dispose() }
