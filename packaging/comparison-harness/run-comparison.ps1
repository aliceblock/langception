# Runs the comparison harness (ticket 01, PP-OCRv6-sidecar feature): the
# bundled PaddleOCR-json engine (what the app drives today) and the oar
# PP-OCRv6 CLI over the fixture screenshots, collecting raw engine output
# and wall-clock timings into results/.
#
#   powershell -File packaging/comparison-harness/run-comparison.ps1
#       [-PaddleDir src-tauri/resources/paddleocr] [-Oar oar]
#       [-Tiers tiny,small,medium] [-SkipPaddle] [-SkipOar]
#
# Outputs, all under results/:
#   paddle_<config>/<image>.json       raw sidecar reply per fixture
#   paddle_<config>/<image>.lines      normalized text lines (scorer input)
#   oar_<tier>/cold/<image>/*.json     raw CLI output per fixture
#   oar_<tier>/<image>.lines           normalized text lines (scorer input)
#   timing.tsv                         one row per timed engine run
#
# The old engine is driven exactly as the app drives it (same spawn args,
# "OCR init completed" handshake, one base64-PNG JSON line per request),
# so its numbers reflect the shipped behavior, including its per-language
# models. zh-hant runs against BOTH config_chinese.txt (what the app's
# Source Language=Chinese actually selects) and the bundle's dedicated
# config_chinese_cht.txt (the old engine's best case for Traditional).

param(
    [string]$PaddleDir = "",
    [string]$Oar = "oar",
    [string]$Tiers = "tiny,small,medium",
    [switch]$SkipPaddle,
    [switch]$SkipOar
)

$ErrorActionPreference = "Stop"
$Here = $PSScriptRoot
$FixtureDir = Join-Path $Here "fixtures"
$ResultsDir = Join-Path $Here "results"
New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null

$Utf8 = New-Object System.Text.UTF8Encoding($false)
$InitTimeoutMs = 60000
$RequestTimeoutMs = 30000

if ($PaddleDir -eq "") {
    $PaddleDir = Join-Path (Resolve-Path (Join-Path $Here "../..")).Path `
        "src-tauri/resources/paddleocr"
}

$Images = Get-ChildItem -Path $FixtureDir -Filter "*.png" |
    Where-Object { $_.Name -ne "bg-art.png" } |
    Sort-Object Name
if ($Images.Count -eq 0) { throw "no fixture images found in $FixtureDir" }

$Timing = New-Object System.Collections.Generic.List[object]

# One timing row per timed run. Meta rows (init, warm averages) carry a
# non-image marker in `image` or `variant` - "(init)", "(batch N)", and a
# "warm-avg" variant suffix - which is how score-comparison.ps1 excludes
# them from per-image means.
function Add-Timing([string]$Engine, [string]$Variant, [string]$Image, [int]$Ms) {
    $Timing.Add([pscustomobject]@{ engine = $Engine; variant = $Variant;
        image = $Image; ms = $Ms })
}

function Save-Lines([string]$Dir, [string]$Name, $Lines) {
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    [System.IO.File]::WriteAllLines((Join-Path $Dir "$Name.lines"), $Lines, $Utf8)
}

function Fresh-Dir([string]$Path) {
    if (Test-Path $Path) { Remove-Item -Recurse -Force $Path }
    New-Item -ItemType Directory -Force -Path $Path | Out-Null
    $Path
}

# ---------------------------------------------------------------------
# Old engine: the bundled PaddleOCR-json sidecar, one process per config
# ---------------------------------------------------------------------

function Run-PaddleConfig([string]$Config, [string]$Variant, $ImageFiles) {
    if ($null -eq $ImageFiles -or $ImageFiles.Count -eq 0) { return }

    $exe = Join-Path $PaddleDir "PaddleOCR-json.exe"
    if (-not (Test-Path $exe)) {
        throw "PaddleOCR-json.exe not found at $exe - the old engine bundle left the repo with pp-ocrv6-sidecar ticket 06 and no release ever shipped it: place a PaddleOCR-json v1.4.1 release archive (hiroi-sora's Umi-OCR distribution) here yourself, or run with -SkipPaddle"
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = "-models_path=models -config_path=models/$Config"
    $psi.WorkingDirectory = $PaddleDir  # configs reference models/ relative paths
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = $Utf8
    $psi.StandardErrorEncoding = $Utf8
    # 0x08000000 = CREATE_NO_WINDOW, same as the app's sidecar spawn.
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)
    # Drain stderr in the background: the engine colorizes and logs a lot,
    # and an unread full pipe would stall it mid-init (the app's spawn
    # does exactly this - paddle.rs).
    $null = $proc.StandardError.ReadToEndAsync()
    $outDir = Join-Path $ResultsDir "paddle_$Variant"
    Fresh-Dir $outDir | Out-Null
    try {
        # Handshake: banner lines, then the readiness marker. EOF (a null
        # ReadLine result) means the process died during init. A pending
        # ReadLineAsync owns the stream, so each line is awaited on the
        # SAME task across poll ticks; never start a second one.
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $task = $proc.StandardOutput.ReadLineAsync()
        while ($true) {
            if (-not $task.Wait(2000)) {
                if ($proc.HasExited) { throw "paddle $Config exited during init" }
                if ($sw.ElapsedMilliseconds -gt $InitTimeoutMs) {
                    throw "paddle $Config did not finish init within ${InitTimeoutMs}ms"
                }
                continue
            }
            $line = $task.Result
            if ($null -eq $line) { throw "paddle $Config exited during init (stdout EOF)" }
            if ($line -match "OCR init completed") { break }
            $task = $proc.StandardOutput.ReadLineAsync()
        }
        $initMs = $sw.ElapsedMilliseconds
        Write-Host ("paddle {0}: init {1}ms" -f $Variant, $initMs)
        Add-Timing "paddle" $Variant "(init)" $initMs

        foreach ($img in $ImageFiles) {
            $b64 = [Convert]::ToBase64String([System.IO.File]::ReadAllBytes($img.FullName))
            $payload = '{"image_base64": "' + $b64 + '"}'

            $sw.Restart()
            $proc.StandardInput.WriteLine($payload)
            $proc.StandardInput.Flush()
            $task = $proc.StandardOutput.ReadLineAsync()
            if (-not $task.Wait($RequestTimeoutMs)) {
                throw "paddle $Config did not reply to $($img.Name) within ${RequestTimeoutMs}ms"
            }
            $reply = $task.Result
            if ($null -eq $reply) { throw "paddle $Config died on $($img.Name) (stdout EOF)" }
            $ms = $sw.ElapsedMilliseconds

            [System.IO.File]::WriteAllText((Join-Path $outDir ($img.BaseName + ".json")),
                $reply, $Utf8)
            # Normalize the reply into text lines for the scorer.
            $lines = @()
            try {
                $json = $reply | ConvertFrom-Json
                if ($json.code -eq 100 -and $json.data) {
                    $lines = @($json.data | ForEach-Object { $_.text })
                }
            } catch {
                Write-Warning "unparsable paddle reply for $($img.Name): $reply"
            }
            Save-Lines $outDir $img.BaseName $lines
            Write-Host ("paddle {0}: {1} {2}ms, {3} lines" -f $Variant, $img.Name, $ms, $lines.Count)
            Add-Timing "paddle" $Variant $img.Name $ms
        }
    } finally {
        if (-not $proc.HasExited) { $proc.Kill() }
        $proc.Dispose()
    }
}

if (-not $SkipPaddle) {
    $byLang = @{}
    foreach ($img in $Images) {
        $parts = $img.BaseName -split "-"
        $lang = if ($parts[0] -eq "zh") { $parts[0..1] -join "-" } else { $parts[0] }
        if (-not $byLang.ContainsKey($lang)) { $byLang[$lang] = @() }
        $byLang[$lang] += $img
    }
    # Source Language -> config mapping is the app's (paddle.rs).
    Run-PaddleConfig "config_en.txt" "en" $byLang["en"]
    Run-PaddleConfig "config_japan.txt" "ja" $byLang["ja"]
    Run-PaddleConfig "config_chinese.txt" "zh-hans" $byLang["zh-hans"]
    # Traditional Chinese: the app's Zh selection first, then the bundle's
    # dedicated CHT config (the old engine's best case).
    Run-PaddleConfig "config_chinese.txt" "zh-hant" $byLang["zh-hant"]
    Run-PaddleConfig "config_chinese_cht.txt" "zh-hant-cht" $byLang["zh-hant"]
    Run-PaddleConfig "config_korean.txt" "ko" $byLang["ko"]
}

# ---------------------------------------------------------------------
# New pipeline: the oar PP-OCRv6 CLI, one timed pass per tier
# ---------------------------------------------------------------------

# One timed oar invocation over one image, its JSON output normalized to
# text lines (text_regions[].text) and saved under $SaveDir. Returns the
# lines; timing is recorded by the caller. The CLI refuses to overwrite,
# so every invocation needs a fresh output directory.
function Invoke-OarJson([string]$Image, [string[]]$ModelArgs, [string]$OutDir) {
    Fresh-Dir $OutDir | Out-Null
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Oar ocr $Image @ModelArgs --device cpu --format json -o $OutDir | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "oar failed on $Image (args: $ModelArgs)" }
    $ms = $sw.ElapsedMilliseconds

    $lines = @()
    $stem = [System.IO.Path]::GetFileNameWithoutExtension($Image)
    $jsonPath = Join-Path $OutDir ($stem + ".json")
    if (Test-Path $jsonPath) {
        $json = [System.IO.File]::ReadAllText($jsonPath, $Utf8) | ConvertFrom-Json
        if ($json.text_regions) {
            $lines = @($json.text_regions | ForEach-Object { $_.text })
        }
    } else {
        Write-Warning "oar wrote no JSON for $Image"
    }
    [pscustomobject]@{ lines = $lines; ms = $ms }
}

function Run-OarTier([string]$Tier) {
    $tierDir = Fresh-Dir (Join-Path $ResultsDir "oar_$Tier")

    # Untimed prefetch: the first ever run downloads/loads the models (the
    # ticket's "hand-fetched" models - fetched once, cached by the CLI).
    & $Oar ocr $Images[0].FullName --size $Tier --device cpu | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "oar prefetch failed for tier $Tier" }

    # Cold per-image: one invocation per image (process + model load +
    # inference), capturing the JSON output for scoring.
    foreach ($img in $Images) {
        $run = Invoke-OarJson $img.FullName @("--size", $Tier) `
            (Join-Path $tierDir "cold/$($img.BaseName)")
        if ($img.BaseName -like "ko-*") {
            # The plain-v6 run on hangul documents the unsupported
            # combination (empty/garbage output); the tier's scored Korean
            # result comes from the v5-korean-rec run below.
            Save-Lines $tierDir "$($img.BaseName)-v6vocab" $run.lines
            Write-Host ("oar {0}: {1} {2}ms (cold), {3} lines [v6 vocab, no hangul]" -f
                $Tier, $img.Name, $run.ms, $run.lines.Count)
            Add-Timing "oar" $Tier "$($img.Name) (v6-vocab)" $run.ms
            continue
        }
        Save-Lines $tierDir $img.BaseName $run.lines
        Write-Host ("oar {0}: {1} {2}ms (cold), {3} lines" -f
            $Tier, $img.Name, $run.ms, $run.lines.Count)
        Add-Timing "oar" $Tier $img.Name $run.ms
    }

    # Korean is the product's documented exception (spec + ADR-0005): the
    # PP-OCRv6 vocabulary has no hangul, so every tier reads Korean via
    # the official Korean PP-OCRv5 recognition model on the tier's v6
    # detection. Run the ko fixtures that way and use it as their result.
    $koArgs = @("--det", "pp-ocrv6_$($Tier)_det.onnx",
        "--rec", "korean_pp-ocrv5_mobile_rec.onnx",
        "--dict", "ppocrv5_korean_dict.txt")
    $koImages = $Images | Where-Object { $_.BaseName -like "ko-*" }
    foreach ($img in $koImages) {
        # Untimed prefetch of the Korean rec model + dictionary.
        & $Oar ocr $img.FullName @koArgs --device cpu | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "oar $Tier korean prefetch failed" }

        $run = Invoke-OarJson $img.FullName $koArgs `
            (Join-Path $tierDir "cold-ko/$($img.BaseName)")
        Save-Lines $tierDir $img.BaseName $run.lines
        Write-Host ("oar {0} ko (v6 det + v5-korean rec): {1} {2}ms (cold), {3} lines" -f
            $Tier, $img.Name, $run.ms, $run.lines.Count)
        Add-Timing "oar" $Tier $img.Name $run.ms
    }

    # Warm marginal: all images in ONE invocation (one model load). From
    # cold_i = L + infer_i and batch = L + sum(infer_i), the per-image
    # marginal inference cost is (batch - cold_avg) / (n - 1) - the load
    # L amortizes away in a long-lived process, which is how the app
    # drives a sidecar. The cold average excludes the v6-vocab KO rows:
    # the same image timed twice would double-count it.
    $warmDir = Fresh-Dir (Join-Path $tierDir "warm")
    $all = $Images | ForEach-Object { $_.FullName }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Oar ocr @all --size $Tier --device cpu --format json -o $warmDir | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "oar $Tier warm batch failed" }
    $batchMs = $sw.ElapsedMilliseconds
    $coldStats = $Timing | Where-Object {
        $_.engine -eq "oar" -and $_.variant -eq $Tier -and $_.image -notlike "*v6-vocab*"
    } | Measure-Object ms -Average
    $warmAvg = [int](($batchMs - $coldStats.Average) / ($Images.Count - 1))
    Write-Host ("oar {0}: cold avg {1}ms, batch({2}) {3}ms -> warm ~{4}ms/image" -f
        $Tier, [int]$coldStats.Average, $Images.Count, $batchMs, $warmAvg)
    Add-Timing "oar" "$Tier warm-avg" "(batch $($Images.Count))" $warmAvg
}

if (-not $SkipOar) {
    foreach ($tier in $Tiers.Split(",")) {
        Run-OarTier $tier.Trim()
    }
}

# ---------------------------------------------------------------------

$timingPath = Join-Path $ResultsDir "timing.tsv"
$rows = @("engine`tvariant`timage`tms")
foreach ($t in $Timing) {
    $rows += ("{0}`t{1}`t{2}`t{3}" -f $t.engine, $t.variant, $t.image, $t.ms)
}
[System.IO.File]::WriteAllLines($timingPath, $rows, $Utf8)
Write-Host "wrote $timingPath"
