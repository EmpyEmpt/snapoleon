<#
 Builds the portable zip: snapoleon.exe plus an empty `portable` marker file.
 snap_core::paths detects the marker next to the executable and keeps config, data,
 history and captures in .\data instead of %APPDATA% / %LOCALAPPDATA%.

 usage: make-portable.ps1 -Exe target\release\snapoleon.exe -Version 0.1.0 -Arch x64 -Out dist
#>
param(
  [Parameter(Mandatory)] [string] $Exe,
  [Parameter(Mandatory)] [string] $Version,
  [string] $Arch = "x64",
  [string] $Out = "dist"
)
$ErrorActionPreference = "Stop"
$name = "Snapoleon-$Version-windows-$Arch-portable"
$stage = Join-Path ([IO.Path]::GetTempPath()) $name
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
$dir = Join-Path $stage "Snapoleon"
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item $Exe (Join-Path $dir "snapoleon.exe")
New-Item -ItemType File (Join-Path $dir "portable") | Out-Null
Copy-Item (Join-Path $PSScriptRoot "..\..\LICENSE-MIT") $dir
Copy-Item (Join-Path $PSScriptRoot "..\..\LICENSE-APACHE") $dir
$readme = @(
  "Snapoleon portable",
  "Run snapoleon.exe. Settings, history and captures are stored in the 'data' folder next to it.",
  "Delete the 'portable' file to switch to a normal per-user profile."
)
Set-Content (Join-Path $dir "README-portable.txt") $readme
New-Item -ItemType Directory -Force $Out | Out-Null
$zip = Join-Path $Out "$name.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $dir -DestinationPath $zip
Write-Host $zip
