use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Privacy-safe IME telemetry event (never records raw IME strings or keystrokes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImeEventKind {
    MarkedUpdate { utf16_len: usize },
    Committed { utf16_len: usize },
    Unmarked,
    Backspace,
    EnterCommittedLine,
    EscapeClearedMark,
    WindowBlur { had_mark: bool },
    WindowFocus { has_mark: bool },
}

/// Core UTF-8 / UTF-16 IME buffer backing GPUI's `EntityInputHandler`.
/// Keeps preedit (`marked_range`) distinct from committed text so macOS IME / macSKK
/// composition, candidate positioning, Escape cancellation, and focus transitions can
/// be verified deterministically.
#[derive(Debug, Clone, Default)]
pub struct ImeBuffer {
    pub content: String,
    pub selected_range: Range<usize>,
    pub marked_range: Option<Range<usize>>,
    pub committed_lines: Vec<String>,
    pub recent_events: Vec<ImeEventKind>,
}

impl ImeBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&mut self, ev: ImeEventKind) {
        self.recent_events.push(ev);
        if self.recent_events.len() > 24 {
            let drain = self.recent_events.len() - 24;
            self.recent_events.drain(..drain);
        }
    }

    pub fn is_composing(&self) -> bool {
        self.marked_range
            .as_ref()
            .is_some_and(|range| !range.is_empty())
    }

    pub fn offset_to_utf16(&self, utf8_offset: usize) -> usize {
        let clamped = self.clamp_to_char_boundary(utf8_offset);
        self.content[..clamped].encode_utf16().count()
    }

    pub fn offset_from_utf16(&self, utf16_offset: usize) -> usize {
        let mut utf16_count = 0;
        for (byte_idx, ch) in self.content.char_indices() {
            if utf16_count >= utf16_offset {
                return byte_idx;
            }
            utf16_count += ch.len_utf16();
        }
        self.content.len()
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn clamp_to_char_boundary(&self, mut offset: usize) -> usize {
        offset = offset.min(self.content.len());
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub fn text_for_range_utf16(
        &self,
        range_utf16: Range<usize>,
    ) -> (Option<String>, Range<usize>) {
        let range = self.range_from_utf16(&range_utf16);
        let adjusted = self.range_to_utf16(&range);
        (Some(self.content[range].to_string()), adjusted)
    }

    pub fn selected_range_utf16(&self) -> Range<usize> {
        self.range_to_utf16(&self.selected_range)
    }

    pub fn marked_range_utf16(&self) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    pub fn unmark_text(&mut self) {
        self.marked_range = None;
        self.record(ImeEventKind::Unmarked);
    }

    pub fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content.replace_range(range.clone(), new_text);
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.marked_range = None;
        self.record(ImeEventKind::Committed {
            utf16_len: new_text.encode_utf16().count(),
        });
    }

    pub fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content.replace_range(range.clone(), new_text);
        let mark_end = range.start + new_text.len();
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..mark_end);
        }

        self.selected_range = new_selected_range_utf16
            .map(|rel_utf16| {
                let rel_start = utf16_offset_within(new_text, rel_utf16.start);
                let rel_end = utf16_offset_within(new_text, rel_utf16.end);
                (range.start + rel_start)..(range.start + rel_end)
            })
            .unwrap_or(mark_end..mark_end);

        self.record(ImeEventKind::MarkedUpdate {
            utf16_len: new_text.encode_utf16().count(),
        });
    }

    pub fn backspace(&mut self) {
        if !self.selected_range.is_empty() {
            let range = self.selected_range.clone();
            self.content.replace_range(range.clone(), "");
            self.selected_range = range.start..range.start;
            self.marked_range = None;
            self.record(ImeEventKind::Backspace);
            return;
        }
        if self.selected_range.start == 0 {
            return;
        }
        let prev = self.content[..self.selected_range.start]
            .grapheme_indices(true)
            .next_back()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        self.content.replace_range(prev..self.selected_range.start, "");
        self.selected_range = prev..prev;
        self.marked_range = None;
        self.record(ImeEventKind::Backspace);
    }

    /// Commits current line only when NOT composing (prevents Enter-on-IME-commit leak).
    pub fn submit_line(&mut self) -> bool {
        if self.is_composing() {
            return false;
        }
        let line = std::mem::take(&mut self.content);
        self.committed_lines.push(line);
        if self.committed_lines.len() > 8 {
            let drain = self.committed_lines.len() - 8;
            self.committed_lines.drain(..drain);
        }
        self.selected_range = 0..0;
        self.marked_range = None;
        self.record(ImeEventKind::EnterCommittedLine);
        true
    }

    pub fn cancel_mark_if_any(&mut self) -> bool {
        if let Some(mark) = self.marked_range.take() {
            self.content.replace_range(mark.clone(), "");
            self.selected_range = mark.start..mark.start;
            self.record(ImeEventKind::EscapeClearedMark);
            return true;
        }
        false
    }

    /// Record window deactivation without wiping `marked_range` (preserves macSKK preedit
    /// across Cmd+Tab app switches unless the OS explicitly unmarks/replaces it).
    pub fn on_window_blur(&mut self) {
        let had_mark = self.is_composing();
        self.record(ImeEventKind::WindowBlur { had_mark });
    }

    pub fn on_window_focus(&mut self) {
        let has_mark = self.is_composing();
        self.record(ImeEventKind::WindowFocus { has_mark });
    }
}

fn utf16_offset_within(s: &str, utf16_offset: usize) -> usize {
    let mut count = 0;
    for (byte_idx, ch) in s.char_indices() {
        if count >= utf16_offset {
            return byte_idx;
        }
        count += ch.len_utf16();
    }
    s.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_utf16_surrogate_pairs_and_japanese_composition_commit() {
        let mut buf = ImeBuffer::new();
        buf.replace_text_in_range(None, "🦀");
        // 🦀 is 4 UTF-8 bytes, 2 UTF-16 code units
        assert_eq!(buf.selected_range_utf16(), 2..2);

        // Start Japanese IME composition: "にほんご"
        buf.replace_and_mark_text_in_range(None, "にほんご", Some(0..4));
        assert!(buf.is_composing());
        assert_eq!(buf.marked_range_utf16(), Some(2..6));
        assert_eq!(buf.selected_range_utf16(), 2..6);

        // Enter during composition must NOT submit line
        assert!(!buf.submit_line());
        assert!(buf.committed_lines.is_empty());

        // Commit conversion to "日本語"
        buf.replace_text_in_range(None, "日本語");
        assert!(!buf.is_composing());
        assert_eq!(buf.content, "🦀日本語");
        assert_eq!(buf.marked_range_utf16(), None);
        assert_eq!(buf.selected_range_utf16(), 5..5);

        // Normal Enter after commit submits once
        assert!(buf.submit_line());
        assert_eq!(buf.committed_lines, vec!["🦀日本語"]);
    }

    #[test]
    fn preserves_marked_text_across_window_blur_and_supports_escape_cancel() {
        let mut buf = ImeBuffer::new();
        buf.replace_and_mark_text_in_range(None, "▽じぶん", None);
        assert!(buf.is_composing());

        // Simulate Cmd+Tab away and back (macSKK)
        buf.on_window_blur();
        assert!(buf.is_composing());
        assert_eq!(buf.content, "▽じぶん");
        buf.on_window_focus();
        assert!(buf.is_composing());

        // Escape clears active mark cleanly
        assert!(buf.cancel_mark_if_any());
        assert!(!buf.is_composing());
        assert!(buf.content.is_empty());
    }

    #[test]
    fn backspace_deletes_grapheme_clusters_atomically() {
        let mut buf = ImeBuffer::new();
        buf.replace_text_in_range(None, "Aか\u{3099}👨‍👩‍👧‍👦");
        buf.backspace();
        assert_eq!(buf.content, "Aか\u{3099}");
        buf.backspace();
        assert_eq!(buf.content, "A");
    }
}
