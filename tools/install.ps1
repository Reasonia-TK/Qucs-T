<#
.SYNOPSIS
  Install Qucs-T for the current user (no administrator rights needed).

.DESCRIPTION
  Copies qucs-t.exe to %LOCALAPPDATA%\Programs\qucs-t, adds a Start menu
  shortcut and (with -Associate) opens .qucst files with Qucs-T.
  Run it from the unpacked release folder or from the repository after
  `cargo build --release`.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1
  powershell -ExecutionPolicy Bypass -File install.ps1 -Associate
#>
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA "Programs\qucs-t"),
    [switch]$Associate,
    [switch]$NoShortcut,
    [switch]$AddToPath
)
$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$candidates = @(
    (Join-Path $here "qucs-t.exe"),
    (Join-Path $here "..\qucs-t.exe"),
    (Join-Path $here "..\target\release\qucs-t.exe")
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) { throw "qucs-t.exe not found next to this script (build with: cargo build --release)" }

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
Copy-Item $exe (Join-Path $Prefix "qucs-t.exe") -Force
foreach ($f in @("README.md", "README.en.md", "LICENSE", "CHANGELOG.md")) {
    $src = @((Join-Path $here $f), (Join-Path $here "..\$f")) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if ($src) { Copy-Item $src $Prefix -Force }
}
$target = Join-Path $Prefix "qucs-t.exe"
Write-Host "installed: $target"

if (-not $NoShortcut) {
    $menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Qucs-T.lnk"
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($menu)
    $lnk.TargetPath = $target
    $lnk.Arguments = "gui"
    $lnk.IconLocation = "$target,0"
    $lnk.Description = "Qucs-T thermal network simulator"
    $lnk.Save()
    Write-Host "Start menu: $menu"
}

if ($Associate) {
    # Per-user file association (HKCU)
    $cls = "HKCU:\Software\Classes"
    New-Item -Force "$cls\QucsT.Schematic\shell\open\command" | Out-Null
    Set-ItemProperty "$cls\QucsT.Schematic" -Name "(default)" -Value "Qucs-T thermal schematic"
    New-Item -Force "$cls\QucsT.Schematic\DefaultIcon" | Out-Null
    Set-ItemProperty "$cls\QucsT.Schematic\DefaultIcon" -Name "(default)" -Value "`"$target`",0"
    Set-ItemProperty "$cls\QucsT.Schematic\shell\open\command" -Name "(default)" -Value "`"$target`" gui `"%1`""
    New-Item -Force "$cls\.qucst\OpenWithProgids" | Out-Null
    New-ItemProperty -Force -Path "$cls\.qucst\OpenWithProgids" -Name "QucsT.Schematic" -Value "" -PropertyType String | Out-Null
    Write-Host ".qucst files: 'Open with' -> Qucs-T (choose it once in Explorer to make it the default)"
}

if ($AddToPath) {
    $p = [Environment]::GetEnvironmentVariable("Path", "User")
    if (($p -split ";") -notcontains $Prefix) {
        [Environment]::SetEnvironmentVariable("Path", ($p.TrimEnd(";") + ";" + $Prefix), "User")
        Write-Host "added to the user PATH (new terminals): $Prefix"
    }
}

Write-Host ""
