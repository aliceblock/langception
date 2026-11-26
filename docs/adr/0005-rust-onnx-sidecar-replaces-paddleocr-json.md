# PP-OCRv6 via our own Rust ONNX sidecar replaces the PaddleOCR-json bundle

The bundled offline OCR engine — hiroi-sora's PaddleOCR-json v1.4.1 — has published no release since August 2024 and loads only PP-OCRv2–v4 model architectures, so the app ships 2023-era PP-OCRv3 per-language models. PaddleOCR's current generation, PP-OCRv6 (June 2026), is a unified multilingual det/rec model family in three sizes (tiny/small/medium) with official ONNX variants, and other OCR tools already offer it as a selectable size. We want those models; the dormant sidecar can never load them.

**Decision**: the bundled engine changes implementation underneath. PaddleOCR-json.exe and its runtime (Paddle Inference, mklml, OpenCV) leave the bundle entirely, replaced by a sidecar binary we own: Rust on ONNX Runtime (the `ort` crate), speaking the same JSON-lines pipe protocol as today, so the existing sidecar machinery — spawn, init/request timeouts, crash detection, recovery notice — is reused unchanged. All three v6 tiers ship bundled and are offered in the Engines tab as three explicit engine options — PP-OCRv6 Tiny, PP-OCRv6 Small, PP-OCRv6 Medium — alongside Windows OCR; there is no separate tier setting, and the option label names the tier. Windows OCR stays both the default engine (unchanged since ticket 55) and the broken-bundle fallback; PP-OCRv6 Small is the tier a bundled-engine selection gets when it names no tier.

## Considered options

- **In-process engine** (`ort` inside langception-core, no child process): rejected — it removes the crash isolation the loop's recovery story depends on; a wedged or segfaulting inference becomes a hung tray app instead of a killable, restartable process.
- **RapidOCR (Python) as the sidecar**: rejected — it embeds a Python runtime into the installer to get pipeline code we can equally own in the language the app already speaks.
- **Keep both engines selectable** ("classic" alongside v6): rejected — ~200 MB of dead DLLs in every installer plus permanent double maintenance for a fallback Windows OCR already provides.

## Consequences

- The det/rec pipeline itself is adopted, not hand-written: the sidecar builds on the oar-ocr crate (Apache-2.0, actively maintained, first-class PP-OCRv6 tiny/small/medium support), pinning the exact ONNX Runtime RC it pins. Upstream stall is recoverable — the pipeline code is a readable fallback.
- Korean is a documented exception: PP-OCRv6's vocabulary contains no hangul (verified against the official dictionaries), so the Korean Source Language reads through the official Korean PP-OCRv5 recognition model while detection stays v6. This is why a v5 model ships in a v6 feature.
- The sidecar is pure Rust + ONNX Runtime with no Windows-only engine artifact, removing the main engine-side blocker for the later macOS/Linux builds.
- Model sourcing moves to the official PP-OCRv6 Hugging Face repos; the packaging fetch script is reworked to fetch models from there, keeping the packaging-time (not first-run) download discipline and its hash checks.
  *Amended by ADR-0008: only the Tiny tier's files keep that packaging-time discipline; Small, Medium and the Korean recognizer set arrive as first-use Model Pack downloads, carrying the same pinned URLs and hash checks to download time.*
- No config migration: the default engine stays Windows OCR (ticket 55), and existing installs that explicitly select the bundled engine keep working and land on the Small tier.
