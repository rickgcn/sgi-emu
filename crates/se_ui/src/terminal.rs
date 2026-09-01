//! VT100-compatible terminal parsing and frontend-neutral snapshots.

use std::cmp::Ordering;
use std::pin::Pin;

use vt100::{Color, Parser, Screen};

use crate::bridge::ffi::{TerminalCellDto, TerminalColorDto, TerminalKeyDto, TerminalSnapshotDto};

const COLUMNS: u16 = 80;
const ROWS: u16 = 24;
const SCROLLBACK_ROWS: usize = 10_000;

const ATTRIBUTE_BOLD: u8 = 1;
const ATTRIBUTE_DIM: u8 = 1 << 1;
const ATTRIBUTE_ITALIC: u8 = 1 << 2;
const ATTRIBUTE_UNDERLINE: u8 = 1 << 3;
const ATTRIBUTE_INVERSE: u8 = 1 << 4;

/// Parser state owned by one terminal widget.
pub struct TerminalModel {
    parser: Parser,
}

/// Creates an empty 80-by-24 terminal with retained host scrollback.
#[must_use]
pub fn new_terminal_model() -> Box<TerminalModel> {
    Box::new(TerminalModel {
        parser: Parser::new(ROWS, COLUMNS, SCROLLBACK_ROWS),
    })
}

impl TerminalModel {
    /// Feeds one output batch and returns the resulting visible grid.
    pub fn terminal_feed(mut self: Pin<&mut Self>, bytes: &[u8]) -> TerminalSnapshotDto {
        self.parser.process(bytes);
        self.snapshot_at_current_offset()
    }

    /// Returns the visible grid at a requested offset from the live screen.
    pub fn terminal_snapshot(
        mut self: Pin<&mut Self>,
        scrollback_offset: u32,
    ) -> TerminalSnapshotDto {
        self.parser
            .screen_mut()
            .set_scrollback(scrollback_offset as usize);
        self.snapshot_at_current_offset()
    }

    /// Clears current terminal contents and retained history.
    pub fn terminal_clear(mut self: Pin<&mut Self>) -> TerminalSnapshotDto {
        self.parser = Parser::new(ROWS, COLUMNS, SCROLLBACK_ROWS);
        self.snapshot_at_current_offset()
    }

    /// Encodes one semantic key using the terminal's current input modes.
    #[must_use]
    pub fn terminal_encode_key(&self, key: TerminalKeyDto, value: u8) -> Vec<u8> {
        let screen = self.parser.screen();
        match key {
            TerminalKeyDto::Text => value.is_ascii().then_some(value).into_iter().collect(),
            TerminalKeyDto::Control => control_byte(value).into_iter().collect(),
            TerminalKeyDto::Up => cursor_sequence(screen, b'A'),
            TerminalKeyDto::Down => cursor_sequence(screen, b'B'),
            TerminalKeyDto::Right => cursor_sequence(screen, b'C'),
            TerminalKeyDto::Left => cursor_sequence(screen, b'D'),
            TerminalKeyDto::Keypad => keypad_sequence(screen, value),
            TerminalKeyDto::Pf1 => b"\x1bOP".to_vec(),
            TerminalKeyDto::Pf2 => b"\x1bOQ".to_vec(),
            TerminalKeyDto::Pf3 => b"\x1bOR".to_vec(),
            TerminalKeyDto::Pf4 => b"\x1bOS".to_vec(),
            TerminalKeyDto::Enter => vec![b'\r'],
            TerminalKeyDto::Backspace => vec![0x08],
            TerminalKeyDto::Tab => vec![b'\t'],
            TerminalKeyDto::Escape => vec![0x1b],
            TerminalKeyDto::Delete => vec![0x7f],
            _ => Vec::new(),
        }
    }

    /// Extracts selected text across the retained screen and scrollback.
    pub fn terminal_selection(
        mut self: Pin<&mut Self>,
        start_row: u32,
        start_column: u16,
        end_row: u32,
        end_column: u16,
    ) -> String {
        let original_offset = self.parser.screen().scrollback();
        let history_rows = actual_scrollback_rows(self.parser.screen_mut());
        let total_rows = history_rows + usize::from(ROWS);
        if total_rows == 0 {
            return String::new();
        }

        let mut start = SelectionPoint {
            row: (start_row as usize).min(total_rows - 1),
            column: start_column.min(COLUMNS),
        };
        let mut end = SelectionPoint {
            row: (end_row as usize).min(total_rows - 1),
            column: end_column.min(COLUMNS),
        };
        if compare_points(start, end) == Ordering::Greater {
            std::mem::swap(&mut start, &mut end);
        }

        let mut result = String::new();
        for absolute_row in start.row..=end.row {
            let (visible_row, offset) = visible_location(absolute_row, history_rows);
            self.parser.screen_mut().set_scrollback(offset);
            let first_column = if absolute_row == start.row {
                start.column
            } else {
                0
            };
            let last_column = if absolute_row == end.row {
                end.column
            } else {
                COLUMNS
            };
            if last_column > first_column {
                result.push_str(
                    &self
                        .parser
                        .screen()
                        .rows(first_column, last_column - first_column)
                        .nth(usize::from(visible_row))
                        .unwrap_or_default(),
                );
            }
            if absolute_row != end.row && !self.parser.screen().row_wrapped(visible_row) {
                result.push('\n');
            }
        }
        self.parser.screen_mut().set_scrollback(original_offset);
        result
    }

    fn snapshot_at_current_offset(&mut self) -> TerminalSnapshotDto {
        let requested_offset = self.parser.screen().scrollback();
        let scrollback_rows = actual_scrollback_rows(self.parser.screen_mut());
        self.parser.screen_mut().set_scrollback(requested_offset);
        let screen = self.parser.screen();
        let scrollback_offset = screen.scrollback();
        let mut cells = Vec::with_capacity(usize::from(COLUMNS) * usize::from(ROWS));
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                cells.push(cell_dto(screen.cell(row, column)));
            }
        }
        let (cursor_row, cursor_column) = screen.cursor_position();

        TerminalSnapshotDto {
            columns: COLUMNS,
            rows: ROWS,
            cells,
            cursor_row,
            cursor_column,
            cursor_visible: scrollback_offset == 0 && !screen.hide_cursor(),
            scrollback_rows: scrollback_rows as u32,
            scrollback_offset: scrollback_offset as u32,
        }
    }
}

/// Normalizes host clipboard text into one ASCII serial byte batch.
#[must_use]
pub fn normalize_terminal_paste(text: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut carriage_return = false;
    for value in text.chars() {
        match value {
            '\r' => {
                result.push(b'\r');
                carriage_return = true;
            }
            '\n' if carriage_return => carriage_return = false,
            '\n' => result.push(b'\r'),
            value if value.is_ascii() => {
                result.push(value as u8);
                carriage_return = false;
            }
            _ => carriage_return = false,
        }
    }
    result
}

fn actual_scrollback_rows(screen: &mut Screen) -> usize {
    let original = screen.scrollback();
    screen.set_scrollback(usize::MAX);
    let rows = screen.scrollback();
    screen.set_scrollback(original);
    rows
}

fn cell_dto(cell: Option<&vt100::Cell>) -> TerminalCellDto {
    let Some(cell) = cell else {
        return TerminalCellDto {
            text: String::new(),
            foreground: color_dto(Color::Default),
            background: color_dto(Color::Default),
            attributes: 0,
        };
    };
    let mut attributes = 0;
    if cell.bold() {
        attributes |= ATTRIBUTE_BOLD;
    }
    if cell.dim() {
        attributes |= ATTRIBUTE_DIM;
    }
    if cell.italic() {
        attributes |= ATTRIBUTE_ITALIC;
    }
    if cell.underline() {
        attributes |= ATTRIBUTE_UNDERLINE;
    }
    if cell.inverse() {
        attributes |= ATTRIBUTE_INVERSE;
    }
    TerminalCellDto {
        text: String::from(cell.contents()),
        foreground: color_dto(cell.fgcolor()),
        background: color_dto(cell.bgcolor()),
        attributes,
    }
}

const fn color_dto(color: Color) -> TerminalColorDto {
    match color {
        Color::Default => TerminalColorDto { kind: 0, value: 0 },
        Color::Idx(value) => TerminalColorDto {
            kind: 1,
            value: value as u32,
        },
        Color::Rgb(red, green, blue) => TerminalColorDto {
            kind: 2,
            value: (red as u32) << 16 | (green as u32) << 8 | blue as u32,
        },
    }
}

fn cursor_sequence(screen: &Screen, final_byte: u8) -> Vec<u8> {
    if screen.application_cursor() {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn keypad_sequence(screen: &Screen, value: u8) -> Vec<u8> {
    if !screen.application_keypad() {
        return match value {
            b'0'..=b'9' | b'.' | b'+' | b'-' | b'*' | b'/' => vec![value],
            b'\r' => vec![b'\r'],
            _ => Vec::new(),
        };
    }
    let final_byte = match value {
        b'0' => b'p',
        b'1' => b'q',
        b'2' => b'r',
        b'3' => b's',
        b'4' => b't',
        b'5' => b'u',
        b'6' => b'v',
        b'7' => b'w',
        b'8' => b'x',
        b'9' => b'y',
        b'.' => b'n',
        b'+' => b'l',
        b'-' => b'm',
        b'*' => b'j',
        b'/' => b'o',
        b'\r' => b'M',
        _ => return Vec::new(),
    };
    vec![0x1b, b'O', final_byte]
}

fn control_byte(value: u8) -> Option<u8> {
    let upper = value.to_ascii_uppercase();
    (b'@'..=b'_').contains(&upper).then_some(upper & 0x1f)
}

#[derive(Clone, Copy)]
struct SelectionPoint {
    row: usize,
    column: u16,
}

fn compare_points(left: SelectionPoint, right: SelectionPoint) -> Ordering {
    match left.row.cmp(&right.row) {
        Ordering::Equal => left.column.cmp(&right.column),
        ordering => ordering,
    }
}

fn visible_location(absolute_row: usize, history_rows: usize) -> (u16, usize) {
    if absolute_row < history_rows {
        (0, history_rows - absolute_row)
    } else {
        ((absolute_row - history_rows) as u16, 0)
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use crate::bridge::ffi::TerminalKeyDto;

    use super::{new_terminal_model, normalize_terminal_paste};

    #[test]
    fn one_feed_produces_a_complete_visible_snapshot() {
        let mut terminal = new_terminal_model();
        let snapshot = Pin::new(&mut *terminal).terminal_feed(b"hello");

        assert_eq!(snapshot.columns, 80);
        assert_eq!(snapshot.rows, 24);
        assert_eq!(snapshot.cells.len(), 80 * 24);
        assert_eq!(snapshot.cells[0].text, "h");
        assert_eq!(snapshot.cells[4].text, "o");
        assert_eq!((snapshot.cursor_row, snapshot.cursor_column), (0, 5));
    }

    #[test]
    fn cursor_and_keypad_modes_change_semantic_key_encoding() {
        let mut terminal = new_terminal_model();
        assert_eq!(
            terminal.terminal_encode_key(TerminalKeyDto::Up, 0),
            b"\x1b[A"
        );
        assert_eq!(
            terminal.terminal_encode_key(TerminalKeyDto::Keypad, b'1'),
            b"1"
        );

        Pin::new(&mut *terminal).terminal_feed(b"\x1b[?1h\x1b=");

        assert_eq!(
            terminal.terminal_encode_key(TerminalKeyDto::Up, 0),
            b"\x1bOA"
        );
        assert_eq!(
            terminal.terminal_encode_key(TerminalKeyDto::Keypad, b'1'),
            b"\x1bOq"
        );
    }

    #[test]
    fn paste_normalizes_newlines_and_ignores_non_ascii_text() {
        assert_eq!(
            normalize_terminal_paste("one\r\ntwo\nthree\rfouré"),
            b"one\rtwo\rthree\rfour"
        );
    }

    #[test]
    fn selection_can_cross_from_scrollback_into_the_live_screen() {
        let mut terminal = new_terminal_model();
        let mut output = String::new();
        for row in 0..26 {
            output.push_str(&format!("row{row:02}\r\n"));
        }
        Pin::new(&mut *terminal).terminal_feed(output.as_bytes());

        let selection = Pin::new(&mut *terminal).terminal_selection(1, 0, 3, 5);

        assert_eq!(selection, "row01\nrow02\nrow03");
    }
}
