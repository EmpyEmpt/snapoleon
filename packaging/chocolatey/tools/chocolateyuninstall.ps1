$ErrorActionPreference = 'Stop'

# The NSIS installer registers a per-user uninstall entry under HKCU.
$keys = Get-UninstallRegistryKey -SoftwareName 'Snapoleon*'

if ($keys.Count -eq 1) {
  $key = $keys[0]
  $uninstaller = ($key.UninstallString -replace '"', '').Trim()
  Uninstall-ChocolateyPackage -PackageName 'snapoleon' -FileType 'exe' -SilentArgs '/S' -File $uninstaller
} elseif ($keys.Count -eq 0) {
  Write-Warning 'Snapoleon was not found in the installed programs list; nothing to uninstall.'
} else {
  Write-Warning "$($keys.Count) matches found. Not uninstalling to avoid removing the wrong program."
  $keys | ForEach-Object { Write-Warning "- $($_.DisplayName)" }
}
