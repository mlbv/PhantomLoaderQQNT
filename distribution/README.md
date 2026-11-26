<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# PhantomLoaderQQNT offline installer

This bundle installs PhantomLoaderQQNT 1.2.0 with LiteLoader API compatibility 5.2.1 and native API version 1 for QQNT 9.9.38-54985 on Windows x64. It includes the loader, injector, payload, default DLVM capture and local capture reporter, and supports Windows PowerShell 5.1 and later.

Before installation, follow the [release verification guide](https://github.com/mlbv/PhantomLoaderQQNT/blob/main/docs/releases.md) to verify the signed source tag, archive hash and GitHub build attestation.

## Install and launch

Close QQ, extract the archive into a directory separate from both the QQ installation and plugin profile, open PowerShell in the extracted bundle and run:

~~~powershell
.\Install.ps1 -CheckOnly
.\Install.ps1
~~~

`Install.cmd` invokes the installer with the matching PowerShell execution-policy setting, while an explicit invocation can use:

~~~powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\Install.ps1
~~~

The installer resolves QQ through the Tencent QQNT registry entries and `%ProgramFiles%\Tencent\QQNT`; use absolute paths to select another installation or an existing plugin profile:

~~~powershell
.\Install.ps1 -QQRoot 'C:\Program Files\Tencent\QQNT' `
    -InstallDirectory 'C:\Program Files\Tencent\QQNT\PhantomLoaderQQNT' `
    -ProfileDirectory 'D:\LiteLoaderQQNT_data' `
    -DesktopDirectory ([Environment]::GetFolderPath('Desktop'))
~~~

The default runtime directory is `PhantomLoaderQQNT` beneath the QQ root, and the default profile is `%LOCALAPPDATA%\PhantomLoaderQQNT\profile`. Let the installer create a new runtime directory; an existing runtime directory must contain a matching installed bundle and its ownership receipt. The desktop and install-parent directories must already exist when installation starts, while an elevated PowerShell session supplies write access for QQ installations under Program Files.

Installation verifies the version selector and exact file fingerprints in `profile.json`, saves the original package bytes as `original-package.bin`, creates the bootstrap and updates `package.json`. It also creates the **QQNT (PhantomLoader)** desktop shortcut, which starts QQ through the bundled injector; launch then validates the installation and starts the supported executable with its native payload. The shortcut is derived from the installation: install replaces a stale shortcut, and launch repairs or recreates it, so it is not an integrity-checked file.

The payload also applies the exact-build wrapper inspection patches before normal wrapper initialization. These patches are mandatory and independent of the DLVM capture setting; they cover the selected signing-stack, code-integrity, handle-inspection and environment-collection routes, without establishing general undetectability.

This bundle targets `9.9.38-54985` and accepts either the hot-update base `9.9.37-54395` or the full-install base `9.9.38-54985`; the pinned root `QQ.exe` is the one from the full Tencent QQNT 9.9.37-54395 installer. A hot update can advance `curVersion` while retaining the earlier `baseVersion`, `buildId` and root executable, so use `Install.ps1 -CheckOnly` to validate the installation against the bundled profile. A staged update to an unsupported version is skipped by clearing `readyVersion`; a pending update installer is refused.

~~~powershell
# From the installed runtime directory
.\Launch.ps1 -CheckOnly
.\Launch.ps1
~~~

`CheckOnly` performs validation, while `WhatIf` previews the operations performed by install, launch and uninstall. Repeating installation with the same bundle and paths validates the existing installation. Command-line diagnostics include launch status, and the desktop shortcut displays launch errors in a dialog.

## DLVM capture and reporting

Core DLVM capture starts before plugin cleanup and loading and has its own setting. To change enablement, use the DLVM capture setting in loader settings or set `enable_dlvm_capture` in `data/LiteLoader/config.json`, then restart QQ through the PhantomLoader shortcut.

When the guard encounters a matching task, it saves the task under `data/dlvm_capture/captures` before QQ terminates. The launcher waits for process exit and opens the report window for the following outcomes:

| Exit code | Outcome |
| --- | --- |
| 0xD1500001 | Capture saved; QQ terminated. |
| 0xD1500002 | Capture write failed; QQ terminated. |
| 78 | Capture setup failed; QQ terminated. |

A normal exit returns the QQ status to the launcher. For an incident exit, the report window matches captures to the process PID and creation and exit times, retains original captures and prepares ZIP attachments under `captures/reports`. The **Open GitHub issue** action opens a prefilled browser form, after which the user selects the offered ZIP and submits the issue in the browser.

The capture-directory path accepts up to 160 UTF-16 code units, while the complete `QQNT.dll` path accepts up to 259 UTF-16 code units. `ProfileDirectory` and `QQRoot` select the corresponding paths.

## Remove the platform

Close QQ, then run `Uninstall.cmd` or the PowerShell entry point:

~~~powershell
.\Uninstall.ps1 -CheckOnly
.\Uninstall.ps1
~~~

Use `QQRoot` and `InstallDirectory` for custom installation locations. Start outside the installed runtime directory so Windows can remove it. Removal verifies the installed inventory independently of the QQNT version selector and restores the original package only when its current bytes match the installed modification. The profile retains plugins, configuration, captures and report ZIPs.

## Bundle and build

The bundle contains `profile.json` for target fingerprints, `bundle.json` for the file inventory, `payload/` for the launcher and payload, `loader/` for runtime sources and `licenses/` for redistribution notices. `Report-Capture.ps1` and `Incident.Common.psm1` provide the local reporter; the installed runtime adds `state.json` and `original-package.bin`, while editable plugins and user data remain in the profile.

From a source checkout, run:

~~~powershell
npm run build:native
npm run package
~~~

The builder uses `native/build/rust/release/phantomloader.exe` and `phantomloader_payload.dll`, copies the listed runtime assets and license notices, and writes a versioned directory, ZIP and SHA-256 file under `dist`. `LoaderRoot`, `NativeBuildDirectory` and `OutputDirectory` select alternative input and output paths, so each build should use an empty output directory. The launcher, payload and packaged runtime must carry the same profile digest.

PhantomLoaderQQNT software is licensed under GNU LGPL version 3 or later, and documentation under Creative Commons Attribution 4.0 International (CC BY 4.0). License texts and preserved upstream notices are in `licenses`. Corresponding source is available through GitHub's **Source code** downloads for the same release tag. Open the extracted repository directory, run the build commands above, and install the rebuilt bundle normally.
