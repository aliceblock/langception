# Generates the comparison-harness fixture screenshots (ticket 01 of the
# PP-OCRv6-sidecar feature): 1280x720 game-UI-styled panels per Source
# Language, with the exact text drawn recorded in fixtures/ground-truth.tsv
# (derived from content.tsv - the single source of truth). The panels are
# committed, so regenerating them is only needed after editing the TSVs.
#
#   powershell -File packaging/comparison-harness/make-fixtures.ps1
#
# Dialogue panels are drawn over fixtures/bg-art.png (a darkened game
# screenshot crop, committed); pass -ArtSource <png> to build it fresh:
#
#   ... -ArtSource C:\path\to\game-capture.png
#
# The script reads the text/font/layout tables as UTF-8 data files, and the
# script itself is plain ASCII (Windows PowerShell 5.1 reads .ps1 scripts
# in the system codepage unless a BOM is present - same rule as the other
# packaging scripts).

param(
    # A game screenshot to crop the shared dialogue background from
    # (1280x720 at offset 240,140, darkened to ~45%). Ignored when
    # fixtures/bg-art.png already exists.
    [string]$ArtSource = ""
)

$ErrorActionPreference = "Stop"

$Here = $PSScriptRoot
$FixtureDir = Join-Path $Here "fixtures"
$ArtPath = Join-Path $FixtureDir "bg-art.png"

Add-Type -AssemblyName System.Drawing

function Read-Tsv([string]$Path) {
    # PS 5.1's Import-Csv assumes the system codepage without a BOM; the
    # tables are BOM-less UTF-8, so decode explicitly.
    $lines = [System.IO.File]::ReadAllLines($Path, [System.Text.Encoding]::UTF8)
    if ($lines.Count -lt 2) { throw "table $Path has no data rows" }
    $header = $lines[0] -split "`t"
    $rows = @()
    foreach ($line in $lines[1..($lines.Count - 1)]) {
        if ($line.Trim() -eq "") { continue }
        $cells = $line -split "`t"
        $row = @{}
        for ($i = 0; $i -lt $header.Count; $i++) { $row[$header[$i]] = $cells[$i] }
        $rows += [pscustomobject]$row
    }
    $rows
}

# --- The shared dialogue background (once) ---

if (-not (Test-Path $ArtPath)) {
    if ($ArtSource -eq "") {
        throw "fixtures/bg-art.png is missing - pass -ArtSource <game screenshot.png> once to build it"
    }
    $src = [System.Drawing.Bitmap]::FromFile($ArtSource)
    $crop = New-Object System.Drawing.Bitmap(1280, 720)
    $g = [System.Drawing.Graphics]::FromImage($crop)
    # Darken to ~45% so dialogue text stays readable over busy art.
    $matrix = New-Object System.Drawing.Imaging.ColorMatrix
    $matrix.Matrix33 = 1.0
    $matrix.Matrix00 = 0.45; $matrix.Matrix11 = 0.45; $matrix.Matrix22 = 0.45
    $attrs = New-Object System.Drawing.Imaging.ImageAttributes
    $attrs.SetColorMatrix($matrix)
    $dest = New-Object System.Drawing.Rectangle(0, 0, 1280, 720)
    $g.DrawImage($src, $dest, 240, 140, 1280, 720,
        [System.Drawing.GraphicsUnit]::Pixel, $attrs)
    $g.Dispose()
    $crop.Save($ArtPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $src.Dispose()
    Write-Host "built $ArtPath"
}

# --- Render the panels ---

$panels = Read-Tsv (Join-Path $Here "panels.tsv")
$rows = Read-Tsv (Join-Path $Here "content.tsv")

foreach ($panel in $panels) {
    $w = [int]$panel.width
    $h = [int]$panel.height
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

    if ($panel.background -eq "art") {
        $art = [System.Drawing.Bitmap]::FromFile($ArtPath)
        $g.DrawImage($art, 0, 0, $w, $h)
        $art.Dispose()
        # The game's translucent dialogue box behind the bottom lines.
        $box = New-Object System.Drawing.SolidBrush(
            [System.Drawing.Color]::FromArgb(150, 16, 18, 22))
        $g.FillRectangle($box, 40, 556, ($w - 80), 148)
        $box.Dispose()
    } elseif ($panel.background.StartsWith("flat:")) {
        $g.Clear([System.Drawing.ColorTranslator]::FromHtml("#" + $panel.background.Substring(5)))
    } else {
        throw "unknown background '$($panel.background)'"
    }

    foreach ($row in $rows | Where-Object { $_.panel -eq $panel.panel }) {
        $color = [System.Drawing.ColorTranslator]::FromHtml("#" + $row.color)
        $brush = New-Object System.Drawing.SolidBrush($color)
        $font = New-Object System.Drawing.Font($row.font, [float]$row.size,
            [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
        $g.DrawString($row.text, $font, $brush, [float]$row.x, [float]$row.y)
        $font.Dispose(); $brush.Dispose()
    }
    $g.Dispose()

    $out = Join-Path $FixtureDir ($panel.panel + ".png")
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Host "wrote $out"
}

# --- Derive ground truth from the same table (language comes from the
# --- panels table's language column - the single derivation rule) ---

$gtLines = @("image`tlanguage`ttext")
foreach ($row in $rows) {
    $lang = ($panels | Where-Object { $_.panel -eq $row.panel }).language
    $gtLines += ("{0}`t{1}`t{2}" -f $row.panel, $lang, $row.text)
}
$gtPath = Join-Path $FixtureDir "ground-truth.tsv"
[System.IO.File]::WriteAllLines($gtPath, $gtLines,
    (New-Object System.Text.UTF8Encoding($false)))
Write-Host "wrote $gtPath"
