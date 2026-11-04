<#
.SYNOPSIS
  Removes everything register.ps1 added for the current user.
#>
$ErrorActionPreference = 'SilentlyContinue'

$extensions = '.jpg', '.jpeg', '.png', '.gif', '.webp', '.bmp', '.tif', '.tiff'
$progId = 'Pix.Image'
$classes = 'HKCU:\Software\Classes'

foreach ($ext in $extensions) {
    Remove-ItemProperty -Path "$classes\$ext\OpenWithProgids" -Name $progId
}
Remove-Item -Path "$classes\$progId" -Recurse -Force
Remove-Item -Path 'HKCU:\Software\Pix' -Recurse -Force
Remove-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Pix'

Write-Host "Unregistered Pix."
