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
# Build the zip by hand: Windows PowerShell 5.1's Compress-Archive writes backslash entry names.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$fs = [IO.File]::Open($zip, [IO.FileMode]::Create)
$za = New-Object IO.Compression.ZipArchive($fs, [IO.Compression.ZipArchiveMode]::Create)
try {
  foreach ($f in Get-ChildItem $dir -File) {
    $e = $za.CreateEntry("Snapoleon/$($f.Name)", [IO.Compression.CompressionLevel]::Optimal)
    $es = $e.Open(); $in = [IO.File]::OpenRead($f.FullName)
    try { $in.CopyTo($es) } finally { $in.Dispose(); $es.Dispose() }
  }
} finally { $za.Dispose(); $fs.Dispose() }
Write-Host $zip
