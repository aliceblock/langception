# Fetches the PP-OCRv6 models for the new OCR sidecar (pp-ocrv6-sidecar
# tickets 02/03) into src-tauri/resources/pp-ocrv6/ - the flat ten-file
# layout the sidecar's launch contract fixes (ticket 02), so a release
# bundle carries all three tiers plus the Korean model with zero setup.
# Run once before `tauri build` - the folder is git-ignored (the license
# attribution file excepted), so it must be re-fetched in every fresh
# checkout/CI job:
#
#   powershell -File packaging/fetch-pp-ocrv6.ps1
#
# Sources: the seven ONNX models come from PaddlePaddle's official ONNX
# repos on Hugging Face (Apache-2.0, not gated) - the repo's
# `inference.onnx` renamed to the launch contract's name. The official
# repos ship the character dictionaries only inside their configs, so the
# three dictionary text files are the oar-ocr project's republication
# (Apache-2.0, https://www.modelscope.cn/models/greatv/oar-ocr - the same
# mirror the oar-ocr crate's registry pins). Every file is SHA-256
# verified against the hashes below before it is placed; the hashes for
# the six PP-OCRv6 models equal the oar-ocr registry's values (byte-
# identical mirror), while the Korean model pins PaddlePaddle's own ONNX
# export - oar's mirror is their paddle2onnx re-conversion of it,
# byte-different but verified recognition-identical on the comparison
# fixtures (ticket 03).
#
# Re-running is idempotent: present files whose hash matches are skipped,
# a present file that no longer matches is re-fetched, and a download
# that does not match its hash aborts the run with a clear error - a
# half-verified file never lands under the contract name.
#
# License attribution: LICENSE-PP-OCRv6-models.txt (committed) covers all
# ten artifacts.
#
# To bump a model, update its URL and hash together (the hash is of the
# exact upstream bytes - verify provenance when re-deriving one).
#
# NOTE: this file is deliberately plain ASCII - Windows PowerShell 5.1
# reads .ps1 scripts in the system codepage unless a BOM is present, and
# non-ASCII punctuation inside double-quoted strings breaks parsing.

$ErrorActionPreference = "Stop"
# Invoke-WebRequest's progress bar renders per transfer and slows big
# downloads to a crawl on Windows PowerShell 5.1; CI has no console for
# it anyway.
$ProgressPreference = "SilentlyContinue"

$Models = @(
    @{ Name = "pp-ocrv6_tiny_det.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_det_onnx/resolve/main/inference.onnx";
       Sha256 = "193bab7a04fca699a6c82e6abb5b81bdb28177f0abd4062552b04908dafb19f8" },
    @{ Name = "pp-ocrv6_small_det.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_det_onnx/resolve/main/inference.onnx";
       Sha256 = "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e" },
    @{ Name = "pp-ocrv6_medium_det.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_det_onnx/resolve/main/inference.onnx";
       Sha256 = "eb13b44b25bb36f89528b68720af8a61d9cf381176107f465db1757b65d086e1" },
    @{ Name = "pp-ocrv6_tiny_rec.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_tiny_rec_onnx/resolve/main/inference.onnx";
       Sha256 = "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6" },
    @{ Name = "pp-ocrv6_small_rec.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/main/inference.onnx";
       Sha256 = "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634" },
    @{ Name = "pp-ocrv6_medium_rec.onnx";
       Url = "https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_rec_onnx/resolve/main/inference.onnx";
       Sha256 = "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba" },
    @{ Name = "korean_pp-ocrv5_mobile_rec.onnx";
       Url = "https://huggingface.co/PaddlePaddle/korean_PP-OCRv5_mobile_rec_onnx/resolve/main/inference.onnx";
       Sha256 = "92f0b7785e64fc9090106a241cf4c1eb97472824558272751b88a2a4476d3a08" },
    @{ Name = "ppocrv6_tiny_dict.txt";
       Url = "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv6_tiny_dict.txt";
       Sha256 = "c5cbe34ef40c29c4df07ed012bf96569cb69a2d2a01a07027e9f13cb832bd9cd" },
    @{ Name = "ppocrv6_dict.txt";
       Url = "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv6_dict.txt";
       Sha256 = "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d" },
    @{ Name = "ppocrv5_korean_dict.txt";
       Url = "https://www.modelscope.cn/api/v1/models/greatv/oar-ocr/repo?Revision=master&FilePath=ppocrv5_korean_dict.txt";
       Sha256 = "a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7" }
)

$RepoRoot = (Resolve-Path "$PSScriptRoot/..").Path
$DestDir = Join-Path $RepoRoot "src-tauri/resources/pp-ocrv6"
New-Item -ItemType Directory -Force -Path $DestDir | Out-Null

function Get-Sha256 {
    param([string]$Path)
    (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

foreach ($Model in $Models) {
    $Dest = Join-Path $DestDir $Model.Name
    if (Test-Path $Dest) {
        $Actual = Get-Sha256 $Dest
        if ($Actual -eq $Model.Sha256) {
            Write-Host "ok (already present, verified) $($Model.Name)"
            continue
        }
        Write-Host "present but SHA256 mismatch - re-fetching $($Model.Name)"
    }

    # Download beside the destination and verify before placing: a
    # tampered or truncated download never lands under the contract name.
    $Tmp = "$Dest.download"
    try {
        Write-Host "downloading $($Model.Url)"
        Invoke-WebRequest -Uri $Model.Url -OutFile $Tmp
        $Actual = Get-Sha256 $Tmp
        if ($Actual -ne $Model.Sha256) {
            Write-Error "SHA256 mismatch for $($Model.Name): expected $($Model.Sha256), got $Actual - do NOT ship; check the upstream repo for a re-upload and update the hash only after verifying the new file's provenance"
        }
        Move-Item -Path $Tmp -Destination $Dest -Force
        Write-Host "fetched + verified $($Model.Name)"
    } finally {
        if (Test-Path $Tmp) { Remove-Item $Tmp -Force }
    }
}

# Belt and braces: the bundle the launch contract promises is complete
# and every file in it verifies.
foreach ($Model in $Models) {
    $Dest = Join-Path $DestDir $Model.Name
    if (-not (Test-Path $Dest)) {
        Write-Error "bundle incomplete: $($Model.Name) missing after fetch"
    }
    $Actual = Get-Sha256 $Dest
    if ($Actual -ne $Model.Sha256) {
        Write-Error "bundle incomplete: $($Model.Name) does not verify after fetch"
    }
}
Write-Host "bundle complete: $($Models.Count)/$($Models.Count) files verified in $DestDir"
