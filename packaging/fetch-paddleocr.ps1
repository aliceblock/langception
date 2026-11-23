# Fetches the PaddleOCR-json sidecar (ticket 31) into src-tauri/resources/
# so the NSIS installer ships it and the default engine works with zero
# setup. Run once before `tauri build` - the folder is git-ignored, so it
# must be re-fetched in every fresh checkout/CI job:
#
#   powershell -File packaging/fetch-paddleocr.ps1
#
# What lands where: the release archive's top-level contents (the exe, its
# DLLs, and models/) are copied into src-tauri/resources/paddleocr/ - the
# layout the app resolves at runtime (exe + models/ siblings; see
# src-tauri/src/shell.rs resolve_paddleocr).
#
# The archive is verified against the upstream-published SHA256 before it
# is extracted, and the download is skipped when the resources are already
# in place (idempotent for CI caching). The committed
# LICENSE-PaddleOCR-json.txt stays in place (the archive does not contain
# a file of that name).
#
# To switch versions, update BOTH variables below AND the hash (published
# in the release notes of https://github.com/hiroi-sora/PaddleOCR-json).
#
# NOTE: this file is deliberately plain ASCII - Windows PowerShell 5.1
# reads .ps1 scripts in the system codepage unless a BOM is present, and
# non-ASCII punctuation inside double-quoted strings breaks parsing.

$ErrorActionPreference = "Stop"

$Version = "1.4.1"
$ArchiveName = "PaddleOCR-json_v${Version}_windows_x64.7z"
$ArchiveUrl = "https://github.com/hiroi-sora/PaddleOCR-json/releases/download/v${Version}/${ArchiveName}"
$ExpectedSha256 = "C0912A70ACB1F8F18FAFE1F438A2935292A6EC7E2859156FA48A33E91358D71D"

$RepoRoot = (Resolve-Path "$PSScriptRoot/..").Path
$DestDir = Join-Path $RepoRoot "src-tauri/resources/paddleocr"
$Marker = Join-Path $DestDir "PaddleOCR-json.exe"

if (Test-Path $Marker) {
    Write-Host "PaddleOCR-json already present at $DestDir - skipping (delete the folder to re-fetch)"
    exit 0
}

# Extraction needs 7z: Windows' bsdtar (C:\Windows\System32\tar.exe) reads
# .7z archives; CI runners and developer machines have it as `tar`.
$Tar = Join-Path $env:SystemRoot "System32/tar.exe"
if (-not (Test-Path $Tar)) {
    Write-Error "tar.exe not found at $Tar - this script extracts .7z with Windows' built-in bsdtar"
}

$Work = Join-Path ([System.IO.Path]::GetTempPath()) "langception-paddleocr-fetch"
if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force -Path $Work | Out-Null

try {
    Write-Host "downloading $ArchiveUrl"
    $Archive = Join-Path $Work $ArchiveName
    Invoke-WebRequest -Uri $ArchiveUrl -OutFile $Archive

    Write-Host "verifying SHA256"
    $ActualSha256 = (Get-FileHash $Archive -Algorithm SHA256).Hash
    if ($ActualSha256 -ne $ExpectedSha256) {
        Write-Error "SHA256 mismatch: expected $ExpectedSha256, got $ActualSha256 - do NOT proceed; check the release notes for a new hash"
    }

    Write-Host "extracting"
    $ExtractDir = Join-Path $Work "extracted"
    New-Item -ItemType Directory -Force -Path $ExtractDir | Out-Null
    & $Tar -xf $Archive -C $ExtractDir
    if ($LASTEXITCODE -ne 0) { Write-Error "extraction failed (exit $LASTEXITCODE)" }

    # The archive wraps everything in PaddleOCR-json_v<version>/ - accept
    # either that single top dir or a flat layout.
    $Top = Get-ChildItem $ExtractDir | Select-Object -First 1
    $HasTopExe = Test-Path (Join-Path $Top.FullName "PaddleOCR-json.exe")
    $SingleTop = (Get-ChildItem $ExtractDir).Count -eq 1
    if ($SingleTop -and $HasTopExe) {
        $Source = $Top.FullName
    } else {
        $Source = $ExtractDir
    }
    if (-not (Test-Path (Join-Path $Source "PaddleOCR-json.exe"))) {
        Write-Error "PaddleOCR-json.exe not found after extraction - unexpected archive layout"
    }
    if (-not (Test-Path (Join-Path $Source "models"))) {
        Write-Error "models/ not found after extraction - unexpected archive layout"
    }

    New-Item -ItemType Directory -Force -Path $DestDir | Out-Null
    Copy-Item (Join-Path $Source "*") $DestDir -Recurse -Force
    Write-Host "bundled the sidecar into $DestDir"
} finally {
    Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
