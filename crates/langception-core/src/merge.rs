//! Multi-line merge (ticket 23): Windows OCR emits one `TextBlock` per
//! detected line, so a multi-line paragraph — a dialogue box, a subtitle —
//! arrives as N independent blocks and would become N pills, N stability
//! windows, and N Translate Engine calls. This pure pass runs between OCR
//! and the tracking pipeline (classic read path only — the fused engine
//! already reads the zone as a whole) and folds the lines that visually
//! form one paragraph into a single `TextBlock`: one pill, one call.
//!
//! Same seam style as the Auto Colors sampler ([`crate::sample`]): pure
//! geometry over `TextBlock`s, no I/O, unit-tested offline. A line joins
//! the group above it only when BOTH hold:
//!
//! 1. **Paragraph leading.** The line's top edge sits within
//!    [`MAX_LINE_GAP_RATIO`] of the previous LINE's height of that line's
//!    bottom edge (the gap may be negative — glyph boxes touch or overlap
//!    slightly). The ruler is the previous *line*, not the growing group's
//!    union: a union-scaled threshold would loosen with every merged line
//!    and swallow the spacing below the paragraph.
//! 2. **Shared column.** The line's horizontal range overlaps the
//!    previous LINE's range by at least [`MIN_COLUMN_OVERLAP`] of the
//!    narrower line's width — the two lines plausibly wrap in one column.
//!    Justified or indented edges ("nearly aligned" ranges) overlap well
//!    over half; side-by-side rows and staggered (e.g. centered) menu
//!    items overlap barely or not at all and stay separate. Both tests
//!    judge against the previous *line* for the same reason: the union's
//!    growing width would let a staggered stack chain together on overlap
//!    it only has with earlier lines. The heuristic stays conservative:
//!    an ambiguous stack must lose its merge rather than gain one.
//!
//! Text joins with a space for space-delimited sources (EN, KO) and
//! without one for CJK (JA, ZH). Whitespace-only lines are dropped first
//! (the pipeline skips them anyway; dropping here keeps a stray blank line
//! from splitting a paragraph), and the input sorts into reading order so
//! the pass is deterministic regardless of engine emission order.
//! Everything unmergeable passes through unchanged — single-line behavior
//! is identical to no merge at all.

use crate::ports::{SourceLanguage, TextBlock};

/// The largest vertical gap that still reads as paragraph leading, as a
/// fraction of the previous line's height. Dialogue/subtitle leading is a
/// small fraction of the line box; menu rows and separate UI labels space
/// wider than this (the ticket brief's "roughly ≤ ~0.7×").
const MAX_LINE_GAP_RATIO: f64 = 0.7;

/// The smallest horizontal overlap — as a fraction of the narrower line's
/// width — for two stacked lines to count as one column. Paragraph lines
/// share well over half their width; centered/staggered menu items don't.
const MIN_COLUMN_OVERLAP: f64 = 0.5;

/// Merges consecutive OCR lines that visually form one paragraph into
/// single Text Blocks (ticket 23). Whitespace-only lines are dropped, the
/// rest sorts into reading order, and each line joins the group above it
/// when the two tests in the module docs hold — anything else passes
/// through unchanged.
pub fn merge_lines(mut blocks: Vec<TextBlock>, source: SourceLanguage) -> Vec<TextBlock> {
    blocks.retain(|line| !line.text.trim().is_empty());
    blocks.sort_by_key(|line| (line.y, line.x));

    let mut out: Vec<TextBlock> = Vec::new();
    let mut group: Option<Group> = None;
    for line in blocks {
        match group.as_mut() {
            Some(g) if g.accepts(&line) => g.extend(line, source),
            _ => {
                if let Some(g) = group.take() {
                    out.push(g.block);
                }
                group = Some(Group::single(line));
            }
        }
    }
    if let Some(g) = group {
        out.push(g.block);
    }
    out
}

/// The paragraph group under construction: the union `TextBlock` so far
/// (what the pipeline sees) and the last accepted LINE's rect — both merge
/// tests judge the next candidate against that line, never the union: the
/// union's growing height would loosen the gap threshold, its growing
/// width would admit staggered lines the previous line doesn't align with
/// (module docs; the `*_ruler_is_the_last_line` tests pin both).
struct Group {
    block: TextBlock,
    last_line: TextBlock,
}

impl Group {
    fn single(line: TextBlock) -> Self {
        Self {
            last_line: line.clone(),
            block: line,
        }
    }

    /// Whether `line` visually continues this group's paragraph.
    fn accepts(&self, line: &TextBlock) -> bool {
        // Negative gap: the boxes touch or overlap slightly (glyph boxes).
        let gap = line.y - (self.last_line.y + self.last_line.height as i32);
        if f64::from(gap) > MAX_LINE_GAP_RATIO * f64::from(self.last_line.height) {
            return false;
        }
        column_overlap(&self.last_line, line) >= MIN_COLUMN_OVERLAP
    }

    /// Extends the group with an accepted line: union rect, joined text,
    /// and the ruler moves to the new last line.
    fn extend(&mut self, line: TextBlock, source: SourceLanguage) {
        let left = self.block.x.min(line.x);
        let top = self.block.y.min(line.y);
        let right = (self.block.x + self.block.width as i32).max(line.x + line.width as i32);
        let bottom = (self.block.y + self.block.height as i32).max(line.y + line.height as i32);
        self.block.x = left;
        self.block.y = top;
        self.block.width = (right - left) as u32;
        self.block.height = (bottom - top) as u32;
        self.block.text = join_text(&self.block.text, &line.text, source);
        self.last_line = line;
    }
}

/// The two rects' horizontal overlap as a share of the narrower width
/// (0.0 = disjoint columns, 1.0 = the narrower sits inside the wider).
fn column_overlap(a: &TextBlock, b: &TextBlock) -> f64 {
    let left = a.x.max(b.x);
    let right = (a.x + a.width as i32).min(b.x + b.width as i32);
    let overlap = (right - left).max(0);
    let narrower = (a.width as i32).min(b.width as i32).max(1);
    f64::from(overlap) / f64::from(narrower)
}

/// Joins two lines' text for `source`: a space between words of
/// space-delimited scripts (EN, KO), a bare concat for CJK (JA, ZH),
/// where an inter-character space is not part of the writing system.
fn join_text(prev: &str, next: &str, source: SourceLanguage) -> String {
    let (prev, next) = (prev.trim_end(), next.trim_start());
    match source {
        SourceLanguage::En | SourceLanguage::Ko => format!("{prev} {next}"),
        SourceLanguage::Ja | SourceLanguage::Zh => format!("{prev}{next}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standard OCR line: 100×20 box at (x, y).
    fn line(x: i32, y: i32, text: &str) -> TextBlock {
        TextBlock {
            x,
            y,
            width: 100,
            height: 20,
            text: text.to_string(),
        }
    }

    fn line_sized(x: i32, y: i32, w: u32, h: u32, text: &str) -> TextBlock {
        TextBlock {
            x,
            y,
            width: w,
            height: h,
            text: text.to_string(),
        }
    }

    #[test]
    fn closely_stacked_overlapping_lines_merge_into_one_block() {
        let merged = merge_lines(
            vec![line(100, 0, "Hello"), line(100, 24, "World")],
            SourceLanguage::En,
        );
        assert_eq!(merged.len(), 1);
        let b = &merged[0];
        assert_eq!(
            (b.x, b.y, b.width, b.height),
            (100, 0, 100, 44),
            "the merged block spans the union of both line boxes"
        );
        assert_eq!(b.text, "Hello World");
    }

    #[test]
    fn three_line_paragraph_merges_into_one_block() {
        let merged = merge_lines(
            vec![
                line(100, 0, "The journey"),
                line(100, 24, "continues into"),
                line(100, 48, "the deep woods."),
            ],
            SourceLanguage::En,
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(
            (merged[0].x, merged[0].y, merged[0].width, merged[0].height),
            (100, 0, 100, 68),
            "one union rect over all three lines"
        );
        assert_eq!(merged[0].text, "The journey continues into the deep woods.");
    }

    #[test]
    fn horizontally_distant_blocks_on_the_same_row_stay_separate() {
        let out = merge_lines(
            vec![line(0, 0, "Left label"), line(300, 0, "Right label")],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Left label");
        assert_eq!(out[1].text, "Right label");
    }

    #[test]
    fn tight_stack_without_column_overlap_stays_separate() {
        // Menu-like: tight leading, but the items' columns don't line up.
        // Centered items overlapping 40px of the narrower 160px line = 25%
        // (under the column threshold)...
        let out = merge_lines(
            vec![
                line_sized(100, 0, 200, 24, "New Game"),
                line_sized(260, 28, 160, 24, "Load Game"),
            ],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
        // ...and fully disjoint columns are a fortiori separate.
        let out = merge_lines(
            vec![
                line_sized(100, 0, 200, 24, "Options"),
                line_sized(330, 28, 160, 24, "Credits"),
            ],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn en_and_ko_join_with_a_space_ja_and_zh_without() {
        let stacked = |first: &str, second: &str, source| {
            merge_lines(vec![line(0, 0, first), line(0, 24, second)], source)[0].text.clone()
        };
        assert_eq!(
            stacked("Hello", "World", SourceLanguage::En),
            "Hello World"
        );
        assert_eq!(stacked("안녕", "하세요", SourceLanguage::Ko), "안녕 하세요");
        assert_eq!(stacked("こんにちは", "世界", SourceLanguage::Ja), "こんにちは世界");
        assert_eq!(stacked("你好", "世界", SourceLanguage::Zh), "你好世界");
    }

    #[test]
    fn single_block_and_non_mergeable_input_pass_through_unchanged() {
        let one = vec![line(50, 60, "Solo")];
        assert_eq!(merge_lines(one.clone(), SourceLanguage::Ja), one);

        // A wide gap (80px between 20px lines) is separate UI text, not a
        // paragraph: byte-identical pass-through, order included.
        let spread = vec![line(0, 0, "Title"), line(0, 100, "Body")];
        assert_eq!(merge_lines(spread.clone(), SourceLanguage::En), spread);
    }

    #[test]
    fn gap_beyond_the_ratio_breaks_the_group() {
        // 16px of leading between 20px lines (over 0.7×) reads as separate
        // UI rows even though the columns line up perfectly.
        let out = merge_lines(
            vec![line(100, 0, "Health"), line(100, 36, "Mana")],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_gap_ruler_is_the_last_line_not_the_growing_union() {
        // The third line's gap (20px) is under 0.7× the union's height
        // (44px) but over 0.7× the last LINE's height (20px). The union
        // must not loosen the threshold with every merge, or the spacing
        // below a paragraph would swallow whatever follows it.
        let out = merge_lines(
            vec![
                line(100, 0, "One"),
                line(100, 24, "Two"),
                line(100, 64, "Three"),
            ],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "One Two");
        assert_eq!(out[1].text, "Three");
    }

    #[test]
    fn the_column_ruler_is_the_last_line_not_the_growing_union() {
        // The third line overlaps the WIDENING UNION (50% via the first
        // line's wide box) but not the previous line at all — a staggered
        // stack must not chain together on overlap it only has with the
        // union.
        let out = merge_lines(
            vec![
                line_sized(0, 0, 200, 20, "A"),
                line_sized(50, 24, 100, 20, "B"),
                line_sized(150, 48, 100, 20, "C"),
            ],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "A B");
        assert_eq!(out[1].text, "C");
    }

    #[test]
    fn whitespace_only_lines_are_dropped() {
        let out = merge_lines(
            vec![line(0, 0, "Real"), line(0, 100, "   ")],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Real");
    }

    #[test]
    fn reading_order_is_normalized_before_merging() {
        // Engine emission order must not matter: the same paragraph in a
        // scrambled order merges to the same block, joined in reading order.
        let out = merge_lines(
            vec![line(100, 24, "World"), line(100, 0, "Hello")],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Hello World");
    }

    #[test]
    fn overlapping_same_row_boxes_join_into_one_line() {
        // Two boxes on one row whose columns overlap heavily are one text
        // run the engine split in two — the column test merges them.
        let out = merge_lines(
            vec![line(0, 0, "Start"), line(50, 0, "game now")],
            SourceLanguage::En,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Start game now");
    }
}
