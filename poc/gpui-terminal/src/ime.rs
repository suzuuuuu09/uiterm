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

/// Core UTF-8 / UTF-16 IME buffer backing GPUI's `EntityInputHandler` for `--probe-ui`.
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
        offset_from_utf16_in(&self.content, utf16_offset)
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
                let rel_start = offset_from_utf16_in(new_text, rel_utf16.start);
                let rel_end = offset_from_utf16_in(new_text, rel_utf16.end);
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

    pub fn on_window_blur(&mut self) {
        let had_mark = self.is_composing();
        self.record(ImeEventKind::WindowBlur { had_mark });
    }

    pub fn on_window_focus(&mut self) {
        let has_mark = self.is_composing();
        self.record(ImeEventKind::WindowFocus { has_mark });
    }
}

/// Live-terminal IME preedit state backing GPUI `EntityInputHandler`.
///
/// Unlike `ImeBuffer` (which holds a toy editable line), the live terminal owns no
/// committed-text buffer—committed text is returned immediately to be written to the
/// PTY as `FRAME_DATA`, while only active preedit (`marked_range`) lives here.
/// Window blur preserves active preedit so macSKK retains marked text across Cmd+Tab.
#[derive(Debug, Clone, Default)]
pub struct TerminalImeState {
    pub preedit: String,
    pub selected_range: Range<usize>,
    pub marked_range: Option<Range<usize>>,
    pub recent_events: Vec<ImeEventKind>,
}

impl TerminalImeState {
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
            || !self.preedit.is_empty()
    }

    pub fn preedit_text(&self) -> &str {
        &self.preedit
    }

    pub fn offset_to_utf16(&self, utf8_offset: usize) -> usize {
        let mut clamped = utf8_offset.min(self.preedit.len());
        while clamped > 0 && !self.preedit.is_char_boundary(clamped) {
            clamped -= 1;
        }
        self.preedit[..clamped].encode_utf16().count()
    }

    pub fn offset_from_utf16(&self, utf16_offset: usize) -> usize {
        offset_from_utf16_in(&self.preedit, utf16_offset)
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn text_for_range_utf16(
        &self,
        range_utf16: Range<usize>,
    ) -> (Option<String>, Range<usize>) {
        let range = self.range_from_utf16(&range_utf16);
        let adjusted = self.range_to_utf16(&range);
        (Some(self.preedit[range].to_string()), adjusted)
    }

    pub fn selected_range_utf16(&self) -> Range<usize> {
        self.range_to_utf16(&self.selected_range)
    }

    pub fn marked_range_utf16(&self) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    /// Updates marked preedit without sending anything to the PTY.
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

        let start = range.start.min(self.preedit.len());
        let end = range.end.min(self.preedit.len()).max(start);
        self.preedit.replace_range(start..end, new_text);
        let mark_end = start + new_text.len();

        if self.preedit.is_empty() {
            self.marked_range = None;
            self.selected_range = 0..0;
        } else {
            self.marked_range = Some(0..self.preedit.len());
            self.selected_range = new_selected_range_utf16
                .map(|rel_utf16| {
                    let rel_start = offset_from_utf16_in(new_text, rel_utf16.start);
                    let rel_end = offset_from_utf16_in(new_text, rel_utf16.end);
                    (start + rel_start).min(self.preedit.len())
                        ..(start + rel_end).min(self.preedit.len())
                })
                .unwrap_or(mark_end..mark_end);
        }

        self.record(ImeEventKind::MarkedUpdate {
            utf16_len: new_text.encode_utf16().count(),
        });
    }

    /// Commits IME/text input, clears preedit, and returns the exact committed text
    /// for immediate PTY transmission (never deduplicating repeated commits like "あ"->"あ").
    pub fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
    ) -> Option<String> {
        let utf16_len = new_text.encode_utf16().count();
        if let Some(r_utf16) = range_utf16 {
            let range = self.range_from_utf16(&r_utf16);
            let start = range.start.min(self.preedit.len());
            let end = range.end.min(self.preedit.len()).max(start);
            self.preedit.replace_range(start..end, new_text);
            let committed = std::mem::take(&mut self.preedit);
            self.marked_range = None;
            self.selected_range = 0..0;
            self.record(ImeEventKind::Committed { utf16_len });
            if committed.is_empty() {
                None
            } else {
                Some(committed)
            }
        } else {
            self.preedit.clear();
            self.marked_range = None;
            self.selected_range = 0..0;
            self.record(ImeEventKind::Committed { utf16_len });
            if new_text.is_empty() {
                None
            } else {
                Some(new_text.to_string())
            }
        }
    }

    /// Discards active mark state without leaking uncommitted preedit to the PTY.
    pub fn unmark_text(&mut self) {
        self.preedit.clear();
        self.marked_range = None;
        self.selected_range = 0..0;
        self.record(ImeEventKind::Unmarked);
    }

    /// Explicitly cancels active preedit (e.g. Escape fallback when IME didn't clear).
    pub fn cancel_mark_if_any(&mut self) -> bool {
        if self.is_composing() {
            self.preedit.clear();
            self.marked_range = None;
            self.selected_range = 0..0;
            self.record(ImeEventKind::EscapeClearedMark);
            true
        } else {
            false
        }
    }

    /// Preserves macSKK / IME preedit across window focus loss (Cmd+Tab).
    pub fn on_window_blur(&mut self) {
        let had_mark = self.is_composing();
        self.record(ImeEventKind::WindowBlur { had_mark });
    }

    pub fn on_window_focus(&mut self) {
        let has_mark = self.is_composing();
        self.record(ImeEventKind::WindowFocus { has_mark });
    }
}

fn offset_from_utf16_in(s: &str, utf16_offset: usize) -> usize {
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
        assert_eq!(buf.selected_range_utf16(), 2..2);

        buf.replace_and_mark_text_in_range(None, "にほんご", Some(0..4));
        assert!(buf.is_composing());
        assert_eq!(buf.marked_range_utf16(), Some(2..6));
        assert_eq!(buf.selected_range_utf16(), 2..6);

        assert!(!buf.submit_line());
        assert!(buf.committed_lines.is_empty());

        buf.replace_text_in_range(None, "日本語");
        assert!(!buf.is_composing());
        assert_eq!(buf.content, "🦀日本語");
        assert_eq!(buf.marked_range_utf16(), None);
        assert_eq!(buf.selected_range_utf16(), 5..5);

        assert!(buf.submit_line());
        assert_eq!(buf.committed_lines, vec!["🦀日本語"]);
    }

    #[test]
    fn preserves_marked_text_across_window_blur_and_supports_escape_cancel() {
        let mut buf = ImeBuffer::new();
        buf.replace_and_mark_text_in_range(None, "▽じぶん", None);
        assert!(buf.is_composing());

        buf.on_window_blur();
        assert!(buf.is_composing());
        assert_eq!(buf.content, "▽じぶん");
        buf.on_window_focus();
        assert!(buf.is_composing());

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

    #[test]
    fn live_ime_preserves_macskk_across_cmd_tab_and_allows_repeat_commits() {
        let mut ime = TerminalImeState::new();
        ime.replace_and_mark_text_in_range(None, "▽じぶん", Some(1..4));
        assert!(ime.is_composing());
        assert_eq!(ime.preedit_text(), "▽じぶん");

        // Cmd+Tab away and back
        ime.on_window_blur();
        assert!(ime.is_composing());
        assert_eq!(ime.preedit_text(), "▽じぶん");
        ime.on_window_focus();
        assert!(ime.is_composing());
        assert_eq!(ime.preedit_text(), "▽じぶん");

        // Commit conversion to "自分"
        let committed = ime.replace_text_in_range(None, "自分");
        assert_eq!(committed.as_deref(), Some("自分"));
        assert!(!ime.is_composing());
        assert!(ime.preedit_text().is_empty());

        // Consecutive identical commits within <250ms must both be emitted
        assert_eq!(ime.replace_text_in_range(None, "あ").as_deref(), Some("あ"));
        assert_eq!(ime.replace_text_in_range(None, "あ").as_deref(), Some("あ"));
    }

    #[test]
    fn live_ime_escape_and_unmark_do_not_leak_preedit_or_revive_on_refocus() {
        let mut ime = TerminalImeState::new();
        ime.replace_and_mark_text_in_range(None, "▽キャンセル", None);
        assert!(ime.cancel_mark_if_any());
        assert!(!ime.is_composing());
        assert!(ime.preedit_text().is_empty());

        // Refocus after Escape cancel must NOT revive canceled preedit
        ime.on_window_blur();
        ime.on_window_focus();
        assert!(!ime.is_composing());
        assert!(ime.preedit_text().is_empty());

        // Unmark also clears without committing
        ime.replace_and_mark_text_in_range(None, "未確定", None);
        ime.unmark_text();
        assert!(!ime.is_composing());
        assert!(ime.preedit_text().is_empty());
    }
}
