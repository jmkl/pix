<#
.SYNOPSIS
  Registers Pix as an image viewer for the current user (no admin needed).

.DESCRIPTION
  Adds Pix to the "Open with" list for common image types and to
  Settings > Apps > Default apps. Windows does not let scripts set the default
  app, so finish in Settings by choosing Pix for the image types you want.

.PARAMETER Exe
  Path to pix.exe. Defaults to pix.exe next to this script.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File register.ps1 -Exe C:\Tools\pix.exe
#>
param([string]$Exe = (Join-Path $PSScriptRoot 'pix.exe'))

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Exe)) { throw "pix.exe not found at '$Exe'. Pass -Exe <path>." }
$Exe = (Resolve-Path $Exe).Path

$extensions = '.jpg', '.jpeg', '.png', '.gif', '.webp', '.bmp', '.tif', '.tiff'
$progId = 'Pix.Image'
$classes = 'HKCU:\Software\Classes'

# ProgID: what happens when a file is opened with Pix.
New-Item -Path "$classes\$progId" -Force | Out-Null
Set-ItemProperty -Path "$classes\$progId" -Name '(default)' -Value 'Image (Pix)'
New-Item -Path "$classes\$progId\DefaultIcon" -Force | Out-Null
Set-ItemProperty -Path "$classes\$progId\DefaultIcon" -Name '(default)' -Value "`"$Exe`",0"
New-Item -Path "$classes\$progId\shell\open\command" -Force | Out-Null
Set-ItemProperty -Path "$classes\$progId\shell\open\command" -Name '(default)' -Value "`"$Exe`" `"%1`""

# Offer Pix in the "Open with" list of each extension.
foreach ($ext in $extensions) {
    $key = "$classes\$ext\OpenWithProgids"
    New-Item -Path $key -Force | Out-Null
    New-ItemProperty -Path $key -Name $progId -PropertyType None -Value ([byte[]]@()) -Force | Out-Null
}

# Capabilities: makes Pix appear in Settings > Apps > Default apps.
$cap = 'HKCU:\Software\Pix\Capabilities'
New-Item -Path $cap -Force | Out-Null
Set-ItemProperty -Path $cap -Name 'ApplicationName' -Value 'Pix'
Set-ItemProperty -Path $cap -Name 'ApplicationDescription' -Value 'Minimal fullscreen image viewer'
New-Item -Path "$cap\FileAssociations" -Force | Out-Null
foreach ($ext in $extensions) {
    Set-ItemProperty -Path "$cap\FileAssociations" -Name $ext -Value $progId
}
New-Item -Path 'HKCU:\Software\RegisteredApplications' -Force | Out-Null
Set-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'Pix' -Value 'Software\Pix\Capabilities'

# Tell Explorer that associations changed.
Add-Type -Namespace Win32 -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int e, uint f, IntPtr a, IntPtr b);'
[Win32.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host "Registered Pix ($Exe)."
Write-Host "Next: Settings > Apps > Default apps > Pix, then pick it for the image types you want."
