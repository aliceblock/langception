# Scores the comparison harness results (ticket 01): each engine variant's
# normalized text lines against fixtures/ground-truth.tsv. Quality per
# fixture = character error rate (CER) of each ground-truth line against
# its best-matching recognized line; a line with CER 0 counts as exact.
# Recognition content is what matters downstream (translation), so
# normalization keeps letters, digits, and CJK ideographs/kana/hangul and
# drops case, whitespace, and punctuation variants (fullwidth vs ASCII,
# quote styles, ellipses) - the same string in different punctuation
# scores as correct.
#
#   powershell -File packaging/comparison-harness/score-comparison.ps1
#
# Writes results/summary.tsv: one row per (engine variant, fixture) with
# exact-line count, mean CER, block count, and mean latency.

$ErrorActionPreference = "Stop"
$Here = $PSScriptRoot
$FixtureDir = Join-Path $Here "fixtures"
$ResultsDir = Join-Path $Here "results"

$Utf8 = New-Object System.Text.UTF8Encoding($false)

function Normalize([string]$s) {
    if ($null -eq $s) { return "" }
    $folded = $s.Normalize([System.Text.NormalizationForm]::FormKC).ToLowerInvariant()
    # Keep letters, digits, and marks; drop whitespace and punctuation.
    ($folded -replace "[^\p{L}\p{N}]", "")
}

function Levenshtein([string]$a, [string]$b) {
    if ($a.Length -eq 0) { return $b.Length }
    if ($b.Length -eq 0) { return $a.Length }
    $prev = New-Object int[] ($b.Length + 1)
    $curr = New-Object int[] ($b.Length + 1)
    for ($j = 0; $j -le $b.Length; $j++) { $prev[$j] = $j }
    for ($i = 1; $i -le $a.Length; $i++) {
        $curr[0] = $i
        for ($j = 1; $j -le $b.Length; $j++) {
            $cost = if ($a[$i - 1] -eq $b[$j - 1]) { 0 } else { 1 }
            $curr[$j] = [Math]::Min([Math]::Min($curr[$j - 1] + 1, $prev[$j] + 1),
                $prev[$j - 1] + $cost)
        }
        $tmp = $prev; $prev = $curr; $curr = $tmp
    }
    $prev[$b.Length]
}

# Ground truth: image | language | text (language is authoritative here -
# it comes from panels.tsv via make-fixtures.ps1, never re-derived from
# the image name).
$gt = @{}
foreach ($line in [System.IO.File]::ReadAllLines(
        (Join-Path $FixtureDir "ground-truth.tsv"), $Utf8) | Select-Object -Skip 1) {
    if ($line.Trim() -eq "") { continue }
    $cells = $line -split "`t"
    $image = $cells[0]
    if (-not $gt.ContainsKey($image)) {
        $gt[$image] = @{ language = $cells[1]; lines =
            New-Object System.Collections.Generic.List[object] }
    }
    $gt[$image].lines.Add([pscustomobject]@{ text = $cells[2] })
}

# Latency: mean ms per (engine, variant, image) from timing.tsv. Meta rows
# are excluded from per-image means: the runner marks them with a parenthesized
# image ("(init)", "(batch N)") or a "warm-avg" variant suffix.
$msOf = @{}
$timingPath = Join-Path $ResultsDir "timing.tsv"
if (Test-Path $timingPath) {
    foreach ($line in [System.IO.File]::ReadAllLines($timingPath, $Utf8) | Select-Object -Skip 1) {
        $cells = $line -split "`t"
        if ($cells[2] -like "(*" -or $cells[1] -like "*warm-avg*") { continue }
        # timing.tsv stores the file name with extension ("x.png"); the
        # ground truth keys are bare stems, so the key rebuilds with .png.
        $key = $cells[0] + "/" + $cells[1] + "/" + $cells[2]
        if (-not $msOf.ContainsKey($key)) { $msOf[$key] = New-Object System.Collections.Generic.List[int] }
        $msOf[$key].Add([int]$cells[3])
    }
}

$images = $gt.Keys | Sort-Object
$rows = @("engine`tvariant`timage`tlanguage`tgt_lines`texact_lines`tmean_cer`tblocks`tmean_ms")

# engine variants discovered from the results tree
$variants = @()
Get-ChildItem -Path $ResultsDir -Directory | ForEach-Object {
    if ($_.Name -like "paddle_*") {
        $variants += [pscustomobject]@{ engine = "paddle"; variant = $_.Name.Substring(7);
            dir = $_.FullName }
    } elseif ($_.Name -like "oar_*") {
        $variants += [pscustomobject]@{ engine = "oar"; variant = $_.Name.Substring(4);
            dir = $_.FullName }
    }
}

foreach ($v in $variants) {
    foreach ($image in $images) {
        $linesPath = Join-Path $v.dir ($image + ".lines")
        if (-not (Test-Path $linesPath)) { continue }
        $recognized = [System.IO.File]::ReadAllLines($linesPath, $Utf8) |
            Where-Object { $_.Trim() -ne "" }
        $recognizedN = @($recognized | ForEach-Object { Normalize $_ })

        $exact = 0
        $cerSum = 0.0
        foreach ($truth in $gt[$image].lines) {
            $truthN = Normalize $truth.text
            if ($truthN.Length -eq 0) { continue }
            $best = [int]::MaxValue
            foreach ($rec in $recognizedN) {
                if ($rec.Length -eq 0) { continue }
                $d = Levenshtein $truthN $rec
                if ($d -lt $best) { $best = $d }
                if ($best -eq 0) { break }
            }
            if ($best -eq 0) { $exact++ }
            $cerSum += $best / [double]$truthN.Length
        }
        $gtCount = $gt[$image].lines.Count
        $meanCer = if ($gtCount -gt 0) { [Math]::Round($cerSum / $gtCount, 3) } else { 0 }

        $lang = $gt[$image].language
        $msKey = "$($v.engine)/$($v.variant)/$image.png"
        $meanMs = ""
        if ($msOf.ContainsKey($msKey)) {
            $meanMs = [int](($msOf[$msKey] | Measure-Object -Average).Average)
        }

        $rows += ("{0}`t{1}`t{2}`t{3}`t{4}`t{5}`t{6}`t{7}`t{8}" -f
            $v.engine, $v.variant, $image, $lang, $gtCount, $exact, $meanCer,
            $recognized.Count, $meanMs)
    }
}

$summaryPath = Join-Path $ResultsDir "summary.tsv"
[System.IO.File]::WriteAllLines($summaryPath, $rows, $Utf8)
Write-Host "wrote $summaryPath"
Get-Content $summaryPath -Encoding UTF8 | Format-Table -AutoSize | Out-String -Width 200
