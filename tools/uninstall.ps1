<#
.SYNOPSIS
  Remove a per-user installation made by install.ps1 (settings are kept unless -Purge).
#>
param(
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA "Programs\qucs-t"),
    [switch]$Purge
)
$ErrorActionPreference = "Continue"

$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Qucs-T.lnk"
if (Test-Path $menu) { Remove-Item $menu -Force; Write-Host "removed $menu" }

$cls = "HKCU:\Software\Classes"
if (Test-Path "$cls\QucsT.Schematic") { Remove-Item "$cls\QucsT.Schematic" -Recurse -Force; Write-Host "removed file association" }
if (Test-Path "$cls\.qucst\OpenWithProgids") {
    Remove-ItemProperty -Path "$cls\.qucst\OpenWithProgids" -Name "QucsT.Schematic" -ErrorAction SilentlyContinue
}

$p = [Environment]::GetEnvironmentVariable("Path", "User")
if ($p -and (($p -split ";") -contains $Prefix)) {
    [Environment]::SetEnvironmentVariable("Path", (($p -split ";") | Where-Object { $_ -and $_ -ne $Prefix }) -join ";", "User")
    Write-Host "removed from the user PATH"
}

if (Test-Path $Prefix) { Remove-Item $Prefix -Recurse -Force; Write-Host "removed $Prefix" }
if ($Purge) {
    $cfg = Join-Path $env:APPDATA "qucs-t"
    if (Test-Path $cfg) { Remove-Item $cfg -Recurse -Force; Write-Host "removed settings $cfg" }
}
