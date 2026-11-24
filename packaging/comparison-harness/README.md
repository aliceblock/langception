# Comparison harness — PP-OCRv6 vs the bundled engine

Evidence for ticket 01 of the PP-OCRv6-sidecar feature (spec:
`.scratch/pp-ocrv6-sidecar/`). Runs the currently bundled PaddleOCR-json
engine and the oar PP-OCRv6 pipeline CLI over the fixture screenshots and
scores both against known ground truth. The recorded result and the go/no-go
decision live in the ticket:
`.scratch/pp-ocrv6-sidecar/issues/01-comparison-harness.md`.

This is a packaging-time / dev-machine tool, deliberately **not** a CI seam:
the old engine needs its fetched bundle and the oar CLI downloads real
models (both stay out of CI).

## One-time setup

```powershell
# the old engine's sidecar bundle (git-ignored)
powershell -File packaging/fetch-paddleocr.ps1

# the new pipeline's CLI (installs the `oar` command on PATH)
cargo install oar-ocr-cli
```

The oar CLI fetches each model on first use into its cache (`~/.oar`,
override with `$env:OAR_HOME`) with SHA-256 verification: the three
PP-OCRv6 tiers, plus `korean_pp-ocrv5_mobile_rec.onnx` +
`ppocrv5_korean_dict.txt` for the Korean exception path. All official
PaddlePaddle Apache-2.0 artifacts.

## Run

```powershell
# regenerate fixtures only after editing the TSVs (they are committed)
powershell -File packaging/comparison-harness/make-fixtures.ps1

powershell -File packaging/comparison-harness/run-comparison.ps1     # both engines
powershell -File packaging/comparison-harness/score-comparison.ps1   # quality scores
```

`run-comparison.ps1 -SkipPaddle` / `-SkipOar` / `-Tiers tiny,small` scope a
run. Raw per-image replies, normalized text lines, and timings land in
`results/` (committed: `summary.tsv` + `timing.tsv` are the evidence trail;
regenerating them just overwrites in place).

## The fixtures

Nine 1280x720 panels (`fixtures/*.png`, committed) styled after the app's
primary content — game UI over art, dialogue boxes, small HUD labels:

- `en-dialogue` / `ja-dialogue` / `zh-hans-dialogue` / `zh-hant-dialogue` /
  `ko-dialogue` — three dialogue lines over a darkened game-screenshot crop
  (the shared `bg-art.png`), plus two small label lines.
- `en-small-lowcontrast` / `ja-small-lowcontrast` /
  `zh-hans-small-lowcontrast` / `ko-small-lowcontrast` — 12–14 px HUD
  labels at ~3:1 contrast on a flat dark panel.

Text is rendered with the language's standard Windows UI font (Segoe UI /
Meiryo / Microsoft YaHei / Microsoft JhengHei / Malgun Gothic) at
game-typical sizes (12–18 px).

`panels.tsv` names the panels and their Source Language; `content.tsv` is
the single source of truth for the drawn text: each row is one line
(panel, font, size, color, x, y, text). `make-fixtures.ps1` renders the
PNGs from them and derives `ground-truth.tsv` (image, language, text), so
the panels and the ground truth can never drift apart.

## Scoring

Each engine's recognized lines are normalized (NFKC fold, casefold, keep
letters/digits/CJK/kana/hangul — drop whitespace and punctuation variants,
since 「」 vs [ and …… vs ... are translation-neutral) and each ground-truth
line is scored against its best-matching recognized line: CER (Levenshtein
over the normalized string), exact at CER 0. `summary.tsv` reports exact
lines, mean CER, block count, and mean per-image latency per engine/panel.

## Engine notes

- The old engine is driven exactly as the app drives it: same spawn args,
  `OCR init completed` handshake, one base64-PNG JSON line per request
  (paddle.rs). The app's Source Language=Chinese selects
  `config_chinese.txt`, so zh-hant is scored against that AND against the
  bundle's dedicated `config_chinese_cht.txt` (its best case).
- The v6 CLI is run CPU-only (`--device cpu`) per the spec. Cold = one
  process per image (spawn + model load + inference); warm = marginal
  inference per image inside one batch invocation, approximating the
  long-lived sidecar the app will use. Latency is rough single-machine
  wall time — the Debug Log Window accumulates the real numbers (ticket 47).
- Korean runs with the product's documented mapping: the tier's v6
  detection + `korean_pp-ocrv5_mobile_rec.onnx` (PP-OCRv6 has no hangul —
  the plain-v6 Korean run is kept as `*-v6vocab.lines` evidence of that).
  One CLI limitation: custom `--det/--rec/--dict` runs use generic
  detection thresholds instead of the v6 preset's tuned ones, so the
  future sidecar can only do better than these numbers.
