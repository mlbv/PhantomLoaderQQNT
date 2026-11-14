<!-- SPDX-FileCopyrightText: 2026 movblock -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

# Windows distribution

The offline bundle targets Windows 11 x64 and records its QQNT build, PhantomLoaderQQNT release and LiteLoader API compatibility in the compatibility profile and bundled inventory. It includes the loader, injector, native API payload, default core DLVM capture and local incident reporter, while installation and launch verify the active QQ version, update selector and exact sizes and SHA-256 fingerprints of the supported files.

QQNT hot updates can retain the original installation’s `baseVersion` and `buildId` while advancing `curVersion`, so the profile records supported base/build pairs separately from the active package version. Both validators require an exact listed pair and the supported current version, in addition to the file fingerprints; they do not modify QQNT’s selector.

## Installing a release

Extract the complete archive into a directory separate from both the QQ installation and the plugin profile, close QQ, and run `Install.cmd` with permission to write the installation. To inspect the installation without changing it, use the check-only form:

~~~powershell
.\Install.ps1 -QQRoot 'C:\Program Files\Tencent\QQNT' -CheckOnly
.\Install.ps1 -QQRoot 'C:\Program Files\Tencent\QQNT'
~~~

The installer uses the original `package.json` together with an unused bootstrap path, backs up the original package bytes and changes its entry point; signed QQ binaries remain unchanged. The created **QQNT (PhantomLoader)** shortcut starts the payload, which applies the in-memory verifier patch, supplies the native binding and installs the profiled wrapper inspection patches before normal wrapper initialization. The wrapper patches remain active independently of the DLVM capture setting.

The default runtime directory is `PhantomLoaderQQNT` beneath the QQ root, and the default profile is `%LOCALAPPDATA%\PhantomLoaderQQNT\profile`. Install parameters can select other absolute paths, including an existing compatible plugin profile. The installer reference describes all parameters and ownership checks.

## Default program capture

Core startup enables DLVM capture before plugin cleanup and loading, and the plugin master switch and individual plugin enablement do not change that core setting. To disable capture, use the loader settings or set `"enable_dlvm_capture": false` in `data/LiteLoader/config.json`, then restart QQ through the PhantomLoader shortcut.

When the guard encounters a matching downloaded program, it saves the capture under `<profile>/data/dlvm_capture/captures` and terminates QQ before the protected callback resumes. The launcher waits for process exit and opens the local report window for capture success (`0xD1500001`), capture-write failure (`0xD1500002`) or capture-setup failure (`78`); ordinary exits return the QQ status to the launcher. Disabling capture permits those programs to follow QQNT's normal execution path; the mandatory wrapper inspection patches remain active.

The report window opens a prefilled GitHub issue in the browser, while the user manually attaches the offered ZIP. The reporter retains original captures, stores report ZIPs under `captures/reports` and checks the exited process PID together with its creation and exit times when selecting a capture.

The capture directory accepts at most 160 UTF-16 code units, while the complete `QQNT.dll` path accepts fewer than 260 characters. If those limits are exceeded, use a shorter `-ProfileDirectory` or QQ installation path. The desktop shortcut hides the console while the monitor waits, and launch diagnostics appear in a dialog or command-line output.

## Removal

Run `Uninstall.cmd` or `Uninstall.ps1` from the original bundle or installed runtime. Removal verifies ownership, restores the exact backed-up package bytes and removes the bootstrap, shortcut and owned runtime files, while profile data and plugins remain available.

## Building the bundle

Install the stable Rust MSVC toolchain, Visual Studio x64 C++ build tools, Node.js and Windows PowerShell 5.1 or later, then run the following commands from the repository root:

~~~powershell
npm run build:native
npm run package
~~~

The native build guide describes pinned dependency preparation and offline builds, while the release builder accepts `-LoaderRoot`, `-NativeBuildDirectory` and `-OutputDirectory` overrides. It reads `distribution/profile.json`, checks product and compatibility versions, and requires the compiled launcher and payload to contain the exact profile digest before emitting a versioned ZIP and SHA-256 file under `dist`.

The bundle inventory records every bundled file path, size and SHA-256. Releases contain the runtime sources, core capture, installer entry points, incident reporter and license notices, because capture is part of the core runtime rather than an optional plugin.

Tagged releases are built by [GitHub Actions](../.github/workflows/release.yml). See [release verification](releases.md) for PGP signatures, archive checksums and build provenance.

Corresponding source is available through GitHub's **Source code (zip)** and **Source code (tar.gz)** downloads for the same release tag. Open the extracted repository directory, rebuild with `npm run build:native` and `npm run package`, then install the rebuilt bundle normally; its inventory is generated from the replacement binaries.
