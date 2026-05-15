# CodeFactory dev launcher
# Initializes the MSVC toolchain from D:\BuildTools before running tauri dev.
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
Set-Location $PSScriptRoot
cmd /c "`"D:\BuildTools\VC\Auxiliary\Build\vcvarsall.bat`" x64 && pnpm tauri dev"
