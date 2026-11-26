# Fetches the PP-OCRv6 models and produces everything packaging ships for
# them (deferred-model-packs ticket 05, ADR-0008). One run, three outputs:
#
#   1. The BUNDLE at src-tauri/resources/pp-ocrv6/ - tiny-only since
#      ADR-0008: the three Tiny files (detection + recognition +
#      dictionary) and the generated models-manifest.json, beside the
#      committed license file and the sidecar exe the release workflow
#      copies in. Small, Medium, and the Korean recognizer leave the
#      bundle entirely; a legacy fat checkout is swept down to the
#      tiny-only shape (the app itself is untouched - the sidecar exe is
#      not this script's file).
#   2. The MODELS MANIFEST at src-tauri/resources/pp-ocrv6/models-
#      manifest.json - generated from the same hash pins this script
#      verifies, so bundle contents, pack archives, and pins come from
#      one discipline. Per pack it pins the file list with SHA-256 and
#      byte size, the pack version, and the ordered URL list: the GitHub
#      Releases asset first (the archive output 3, published on the same
#      release by the release workflow), then the pinned upstream per-file
#      URLs. The manifest ships inside the app and is never fetched at
#      runtime (spec story 27); the app's ReadinessGate and PackDownloader
#      read it (langception-core/src/model_packs.rs pins the exact JSON
#      shape in its realistic-manifest test).
#   3. The PACK ARCHIVES in packaging/ - pp-ocrv6-small.zip,
#      pp-ocrv6-medium.zip, pp-ocrv6-korean.zip, each a flat zip of
#      exactly the pack's files (the layout install_pack_archive accepts),
#      built from the same staged files the manifest pins and re-verified
#      entry-by-entry after compression (belt and braces).
#
# Run once before `tauri build` - the bundle's model files are
# git-ignored (the license attribution file excepted), so it must be
# re-fetched in every fresh checkout/CI job:
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
# fixtures (pp-ocrv6-sidecar ticket 03).
#
# All ten files are fetched into a work dir under packaging/stage/
# (git-ignored) even though only Tiny lands in the bundle: the archives
# are built from the same verified bytes the manifest pins. Re-running is
# idempotent: present files whose hash matches are skipped, a present
# file that no longer matches is re-fetched, and a download that does not
# match its hash aborts the run with a clear error - a half-verified
# file never lands anywhere. A run interrupted mid-shrink leaves the app
# working on Windows OCR; re-running heals the bundle.
#
# The manifest's primary asset URL embeds the app version from
# src-tauri/tauri.conf.json - the release workflow checks the tag against
# the same version, so the URLs point at the release that publishes the
# archives. Bump a model by updating its URL and hash together (the hash
# is of the exact upstream bytes - verify provenance when re-deriving
# one); a changed hash re-downloads on the user's machine through the
# stale-file path (spec story 22).
#
# License attribution: LICENSE-PP-OCRv6-models.txt (committed) covers all
# ten artifacts.
#
# NOTE: this file is deliberately plain ASCII - Windows PowerShell 5.1
# reads .ps1 scripts in the system codepage unless a BOM is present, and
# non-ASCII punctuation inside double-quoted strings breaks parsing.

$ErrorActionPreference = "Stop"
# Invoke-WebRequest's progress bar renders per transfer and slows big
# downloads to a crawl on Windows PowerShell 5.1; CI has no console for
# it anyway.
$ProgressPreference = "SilentlyContinue"

# The pins - the manifest's single source. The pack tables below name
# these files; nothing else in the script repeats a hash or URL.
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

# What ships where (ADR-0008): Tiny in the bundle, the rest as Model
# Packs. File order here is the manifest's file-list order (and the
# archive layout's); the shared dictionary repeating across Small and
# Medium is one file with one pin, as the manifest requires.
$TinyFiles = @(
    "pp-ocrv6_tiny_det.onnx",
    "pp-ocrv6_tiny_rec.onnx",
    "ppocrv6_tiny_dict.txt"
)
$Packs = @(
    @{ Name = "small";
       Files = @("pp-ocrv6_small_det.onnx", "pp-ocrv6_small_rec.onnx", "ppocrv6_dict.txt") },
    @{ Name = "medium";
       Files = @("pp-ocrv6_medium_det.onnx", "pp-ocrv6_medium_rec.onnx", "ppocrv6_dict.txt") },
    @{ Name = "korean";
       Files = @("korean_pp-ocrv5_mobile_rec.onnx", "ppocrv5_korean_dict.txt") }
)
# The manifest's per-pack version string. The hashes are the real truth
# (staleness is detected per file); bump this only when you want the
# change visible to humans.
$PackVersion = "1"
# The releases publish the pack archives here; the manifest's primary
# URLs point at these assets.
$RepoSlug = "intception-app/langception"

$RepoRoot = (Resolve-Path "$PSScriptRoot/..").Path
$BundleDir = Join-Path $RepoRoot "src-tauri/resources/pp-ocrv6"
$WorkDir = Join-Path $RepoRoot "packaging/stage/pack-files"
$ConfigPath = Join-Path $RepoRoot "src-tauri/tauri.conf.json"

# The model pins indexed by name - every later step resolves files
# through this, so a name can only come from the one table.
$Pins = @{}
foreach ($Model in $Models) { $Pins[$Model.Name] = $Model }

function Get-Sha256 {
    param([string]$Path)
    (Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# ---- 1. fetch + verify all ten files into the work dir ----

New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
foreach ($Model in $Models) {
    $Dest = Join-Path $WorkDir $Model.Name
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

# Belt and braces: every pin verifies against the work dir before
# anything below may consume it.
foreach ($Model in $Models) {
    $Dest = Join-Path $WorkDir $Model.Name
    if (-not (Test-Path $Dest)) {
        Write-Error "fetch incomplete: $($Model.Name) missing after fetch"
    }
    $Actual = Get-Sha256 $Dest
    if ($Actual -ne $Model.Sha256) {
        Write-Error "fetch incomplete: $($Model.Name) does not verify after fetch"
    }
}
Write-Host "fetch complete: all $($Models.Count) files verified in $WorkDir"

# The app version names the release whose assets the manifest's primary
# URLs point at - the same value the release workflow checks the tag
# against, so the URLs and the published assets cannot drift.
$Config = Get-Content $ConfigPath -Raw | ConvertFrom-Json
$AppVersion = $Config.version
Write-Host "manifest primary URLs point at release v$AppVersion"

function Format-ManifestFile {
    param([string]$Name, [string]$Sha256, [long]$Size)
    $jsonName = $Name.Replace("\", "\\").Replace("`"", "\`"")
    "        {`"name`": `"$jsonName`", `"sha256`": `"$Sha256`", `"size`": $Size}"
}

# The manifest's file-list lines for one group of files (the bundled
# baseline or a pack): pin lookup + verified byte size per file.
function Format-ManifestFileLines {
    param([string[]]$FileNames)
    $Lines = @()
    foreach ($FileName in $FileNames) {
        $pin = $Pins[$FileName]
        $path = Join-Path $WorkDir $FileName
        $Lines += Format-ManifestFile -Name $FileName -Sha256 $pin.Sha256 -Size (Get-Item $path).Length
    }
    $Lines
}

# One pack's manifest body: the release asset first, then each file's
# pinned upstream URL (the table's order), then the file pins.
function Format-ManifestPack {
    param([string]$PackName, [string[]]$FileNames)
    $assetUrl = "https://github.com/$RepoSlug/releases/download/v$AppVersion/pp-ocrv6-$PackName.zip"
    $urlLines = @( "        `"$assetUrl`"" )
    foreach ($FileName in $FileNames) {
        $urlLines += "        `"$($Pins[$FileName].Url)`""
    }
    @(
        "    `"$PackName`": {",
        "      `"version`": `"$PackVersion`",",
        "      `"urls`": [",
        ($urlLines -join ",`n"),
        "      ],",
        "      `"files`": [",
        ((Format-ManifestFileLines -FileNames $FileNames) -join ",`n"),
        "      ]",
        "    }"
    ) -join "`n"
}

# ---- 2. generate the models manifest into the bundle dir ----

$bundledLines = Format-ManifestFileLines -FileNames $TinyFiles
$packBodies = @()
foreach ($Pack in $Packs) {
    $packBodies += Format-ManifestPack -PackName $Pack.Name -FileNames $Pack.Files
}
$Manifest = @(
    "{",
    # Must match MANIFEST_VERSION in crates/langception-core/src/
    # model_packs.rs - a mismatch is the app's named unsupported-version
    # error, never a guess.
    "  `"manifest_version`": 1,",
    "  `"bundled`": {",
    "    `"files`": [",
    ($bundledLines -join ",`n"),
    "    ]",
    "  },",
    "  `"packs`": {",
    ($packBodies -join ",`n"),
    "  }",
    "}"
) -join "`n"

New-Item -ItemType Directory -Force -Path $BundleDir | Out-Null

# A legacy fat checkout shrinks to the tiny-only shape: every known
# model file that is not Tiny leaves the bundle. Only the pinned names
# are swept - the license file and the sidecar exe are not this script's
# to delete.
foreach ($Model in $Models) {
    if ($TinyFiles -contains $Model.Name) { continue }
    $Stale = Join-Path $BundleDir $Model.Name
    if (Test-Path $Stale) {
        Remove-Item $Stale -Force
        Write-Host "swept from the bundle (now a Model Pack file) $($Model.Name)"
    }
}

# Stage the Tiny files: copy the verified bytes and re-verify the copy.
foreach ($FileName in $TinyFiles) {
    $Dest = Join-Path $BundleDir $FileName
    Copy-Item (Join-Path $WorkDir $FileName) $Dest -Force
    $Actual = Get-Sha256 $Dest
    if ($Actual -ne $Pins[$FileName].Sha256) {
        Write-Error "bundle staging failed: copied $FileName does not verify"
    }
    Write-Host "staged (verified) $FileName"
}

# BOM-less UTF-8: the app's manifest reader is serde_json, which rejects
# a byte-order mark. .NET's WriteAllText with an explicit UTF8Encoding
# keeps the BOM off (PowerShell's -Encoding utf8 would write one).
$ManifestPath = Join-Path $BundleDir "models-manifest.json"
[System.IO.File]::WriteAllText($ManifestPath, $Manifest, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "generated models-manifest.json (bundled: $($TinyFiles.Count) files; packs: $($Packs.Count))"

# ---- 3. build + roundtrip-verify the three pack archives ----

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-StreamSha256 {
    param([System.IO.Stream]$Stream)
    $Sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        ([BitConverter]::ToString($Sha.ComputeHash($Stream))).Replace("-", "").ToLowerInvariant()
    } finally {
        $Sha.Dispose()
    }
}

# The archive must hold exactly the pack's files - no extras, no
# missing, no directory noise - and every entry must still hash to its
# pin after compression. This is the same verification the user's
# machine runs at download time (install_pack_archive), applied where
# the archive is born.
function Test-PackArchive {
    param([string]$ZipPath, [string[]]$ExpectedNames)
    $Zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $Entries = @($Zip.Entries | Where-Object { $_.FullName -notmatch "/$" })
        $ActualNames = @($Entries | ForEach-Object { $_.FullName } | Sort-Object)
        $Expected = @($ExpectedNames | Sort-Object)
        if (($ActualNames -join "|") -ne ($Expected -join "|")) {
            Write-Error "archive entry mismatch in ${ZipPath}: expected [$($Expected -join ', ')], got [$($ActualNames -join ', ')] - Compress-Archive must produce flat entries named exactly the model files"
        }
        foreach ($Entry in $Entries) {
            $Pin = $Pins[$Entry.FullName]
            $Stream = $Entry.Open()
            try {
                $Actual = Get-StreamSha256 -Stream $Stream
            } finally {
                $Stream.Dispose()
            }
            if ($Actual -ne $Pin.Sha256) {
                Write-Error "archive entry $($Entry.FullName) in $ZipPath does not verify (expected $($Pin.Sha256), got $Actual)"
            }
            if ($Entry.Length -ne (Get-Item (Join-Path $WorkDir $Entry.FullName)).Length) {
                Write-Error "archive entry $($Entry.FullName) in $ZipPath has the wrong size"
            }
        }
    } finally {
        $Zip.Dispose()
    }
}

foreach ($Pack in $Packs) {
    $StagePack = Join-Path $RepoRoot "packaging/stage/pack-$($Pack.Name)"
    if (Test-Path $StagePack) { Remove-Item $StagePack -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $StagePack | Out-Null
    foreach ($FileName in $Pack.Files) {
        Copy-Item (Join-Path $WorkDir $FileName) (Join-Path $StagePack $FileName) -Force
    }
    # Flat entries: the files sit directly in the staging dir, so the
    # archive is exactly the layout install_pack_archive accepts.
    $ZipPath = Join-Path $RepoRoot "packaging/pp-ocrv6-$($Pack.Name).zip"
    Compress-Archive -Path (Join-Path $StagePack "*") -DestinationPath $ZipPath -Force
    Test-PackArchive -ZipPath $ZipPath -ExpectedNames $Pack.Files
    Write-Host "built + roundtrip-verified pp-ocrv6-$($Pack.Name).zip ($([math]::Round((Get-Item $ZipPath).Length / 1MB, 1)) MB)"
}

Write-Host ""
Write-Host "packaging complete:"
Write-Host "  bundle: $BundleDir (tiny-only + models-manifest.json)"
Get-ChildItem $BundleDir | ForEach-Object { Write-Host "    $($_.Name)" }
Write-Host "  archives:"
Get-ChildItem (Join-Path $RepoRoot "packaging/pp-ocrv6-*.zip") | ForEach-Object {
    Write-Host "    $($_.Name) ($([math]::Round($_.Length / 1MB, 1)) MB)"
}
