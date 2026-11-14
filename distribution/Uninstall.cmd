@echo off
rem SPDX-FileCopyrightText: 2026 movblock
rem SPDX-License-Identifier: LGPL-3.0-or-later

"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0Uninstall.ps1" %*
exit /b %errorlevel%
