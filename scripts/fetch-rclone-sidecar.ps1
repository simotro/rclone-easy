# Scarica il binario ufficiale di rclone (Windows amd64) e lo posiziona
# come sidecar Tauri sotto src-tauri/binaries/, con il suffisso di
# target-triple richiesto dalla convenzione di Tauri per gli externalBin.
# Non è versionato in git (vedi src-tauri/binaries/.gitignore): va
# rieseguito dopo un clone pulito o quando si vuole aggiornare la versione
# inclusa. Equivalente Windows di fetch-rclone-sidecar.sh (vedi lì per i
# commenti sulla convenzione dei sidecar).
$ErrorActionPreference = "Stop"

$TargetTriple = "x86_64-pc-windows-msvc"
$DestDir = Join-Path (Split-Path -Parent $PSScriptRoot) "src-tauri\binaries"
$DestBin = Join-Path $DestDir "rclone-$TargetTriple.exe"
$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())

New-Item -ItemType Directory -Path $TmpDir | Out-Null
try {
    Write-Host "Scarico rclone (windows-amd64)..."
    $ZipPath = Join-Path $TmpDir "rclone.zip"
    Invoke-WebRequest -Uri "https://downloads.rclone.org/rclone-current-windows-amd64.zip" -OutFile $ZipPath

    Write-Host "Estraggo..."
    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir

    $ExtractedBin = Get-ChildItem -Path $TmpDir -Filter "rclone.exe" -Recurse | Select-Object -First 1

    New-Item -ItemType Directory -Path $DestDir -Force | Out-Null
    Copy-Item -Path $ExtractedBin.FullName -Destination $DestBin -Force

    Write-Host "Installato in $DestBin"
    & $DestBin version | Select-Object -First 1
} finally {
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
