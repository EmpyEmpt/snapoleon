$ErrorActionPreference = 'Stop'

$packageArgs = @{
  packageName    = 'snapoleon'
  fileType       = 'exe'
  url64bit       = '${{ BASE_URL }}/Snapoleon-${{ VERSION }}-windows-x64-setup.exe'
  checksum64     = '${{ SHA256_WINDOWS_X64_SETUP }}'
  checksumType64 = 'sha256'
  silentArgs     = '/S'
  validExitCodes = @(0)
}

# The ARM64 build is selected automatically on Windows on ARM.
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
  $packageArgs['url64bit']   = '${{ BASE_URL }}/Snapoleon-${{ VERSION }}-windows-arm64-setup.exe'
  $packageArgs['checksum64'] = '${{ SHA256_WINDOWS_ARM64_SETUP }}'
}

Install-ChocolateyPackage @packageArgs
