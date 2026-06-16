# Fetches the bundled FFmpeg shared build (NVENC/AMF/QSV) into src-tauri/ffmpeg.
#
# We pin FFmpeg 8.1 (BtbN gpl-shared) to match rusty_ffmpeg's prebuilt binding
# (src-tauri/ffmpeg/binding.rs, committed). Run once after cloning.
#
#   pwsh -File scripts/fetch-ffmpeg.ps1

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$version = "n8.1"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-$version-latest-win64-gpl-shared-8.1.zip"
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root "src-tauri\ffmpeg"
$zip = Join-Path $env:TEMP "hako-ffmpeg-$version.zip"
$tmp = Join-Path $env:TEMP "hako-ffmpeg-$version"

Write-Host "Downloading FFmpeg $version (gpl-shared, win64)..."
Invoke-WebRequest -Uri $url -OutFile $zip
Write-Host ("  {0:N1} MB" -f ((Get-Item $zip).Length / 1MB))

if (Test-Path $tmp) { Remove-Item $tmp -Recurse -Force }
Expand-Archive -Path $zip -DestinationPath $tmp -Force
$top = Get-ChildItem $tmp -Directory | Select-Object -First 1

New-Item -ItemType Directory -Path $dest -Force | Out-Null
foreach ($d in @("bin", "include", "lib")) {
    $target = Join-Path $dest $d
    if (Test-Path $target) { Remove-Item $target -Recurse -Force }
    Copy-Item (Join-Path $top.FullName $d) $target -Recurse -Force
}
Copy-Item (Join-Path $top.FullName "LICENSE.txt") (Join-Path $dest "LICENSE.txt") -Force

Remove-Item $zip -Force
Remove-Item $tmp -Recurse -Force

$nvenc = & (Join-Path $dest "bin\ffmpeg.exe") -hide_banner -encoders 2>&1 |
    Select-String "h264_nvenc"
Write-Host "FFmpeg installed to src-tauri/ffmpeg"
Write-Host ("  h264_nvenc present: {0}" -f [bool]$nvenc)
if (-not (Test-Path (Join-Path $dest "binding.rs"))) {
    Write-Warning "src-tauri/ffmpeg/binding.rs missing — it is committed to the repo; restore it from version control."
}
