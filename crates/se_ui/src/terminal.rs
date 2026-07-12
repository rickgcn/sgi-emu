//! VT100 terminal state and the Qt-facing serial transport bridge.

use crate::application::ffi;

const TERMINAL_ROWS: u16 = 24;
const TERMINAL_COLUMNS: u16 = 80;
const TERMINAL_SCROLLBACK_LINES: usize = 10_000;

#[derive(Default)]
struct TerminalCallbacks {
    bell_count: u64,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell_count = self.bell_count.saturating_add(1);
    }

    fn visual_bell(&mut self, _: &mut vt100::Screen) {
        self.bell_count = self.bell_count.saturating_add(1);
    }
}

type Parser = vt100::Parser<TerminalCallbacks>;

/// Two independent VT100 screens connected to the IP32 serial ports.
pub struct TerminalModel {
    parsers: [Parser; 2],
}

impl TerminalModel {
    fn parser(&self, port: ffi::UiSerialPort) -> &Parser {
        &self.parsers[port_index(port)]
    }

    fn parser_mut(&mut self, port: ffi::UiSerialPort) -> &mut Parser {
        &mut self.parsers[port_index(port)]
    }

    pub fn process_output(&mut self, port: ffi::UiSerialPort, bytes: &[u8]) {
        self.parser_mut(port).process(bytes);
    }

    pub fn clear(&mut self, port: ffi::UiSerialPort) {
        self.parsers[port_index(port)] = new_parser();
    }

    pub fn clear_all(&mut self) {
        self.parsers = [new_parser(), new_parser()];
    }

    pub fn set_scrollback(&mut self, port: ffi::UiSerialPort, rows: usize) {
        self.parser_mut(port).screen_mut().set_scrollback(rows);
    }

    pub fn snapshot(&mut self, port: ffi::UiSerialPort) -> ffi::UiTerminalSnapshot {
        let parser = self.parser_mut(port);
        let current_scrollback = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(usize::MAX);
        let maximum_scrollback = parser.screen().scrollback();
        parser
            .screen_mut()
            .set_scrollback(current_scrollback.min(maximum_scrollback));

        let screen = parser.screen();
        let (rows, columns) = screen.size();
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(columns));
        for row in 0..rows {
            for column in 0..columns {
                cells.push(
                    screen
                        .cell(row, column)
                        .map(cell_from_vt100)
                        .unwrap_or_else(empty_cell),
                );
            }
        }
        let (cursor_row, cursor_column) = screen.cursor_position();
        ffi::UiTerminalSnapshot {
            rows,
            columns,
            cells,
            cursor_row,
            cursor_column,
            cursor_visible: !screen.hide_cursor() && screen.scrollback() == 0,
            scrollback: screen.scrollback(),
            maximum_scrollback,
            bell_count: parser.callbacks().bell_count,
        }
    }

    pub fn selected_text(
        &self,
        port: ffi::UiSerialPort,
        start_row: u16,
        start_column: u16,
        end_row: u16,
        end_column: u16,
    ) -> String {
        self.parser(port)
            .screen()
            .contents_between(start_row, start_column, end_row, end_column)
    }

    pub fn encode_key(
        &self,
        port: ffi::UiSerialPort,
        key: ffi::UiTerminalKey,
        text: &str,
        control: bool,
        alt: bool,
    ) -> Vec<u8> {
        let application_cursor = self.parser(port).screen().application_cursor();
        let mut bytes = match key {
            ffi::UiTerminalKey::Text => encode_text(text, control),
            ffi::UiTerminalKey::Enter => vec![b'\r'],
            ffi::UiTerminalKey::Backspace => vec![0x7f],
            ffi::UiTerminalKey::Tab => vec![b'\t'],
            ffi::UiTerminalKey::Escape => vec![0x1b],
            ffi::UiTerminalKey::Up => cursor_sequence(application_cursor, b'A'),
            ffi::UiTerminalKey::Down => cursor_sequence(application_cursor, b'B'),
            ffi::UiTerminalKey::Right => cursor_sequence(application_cursor, b'C'),
            ffi::UiTerminalKey::Left => cursor_sequence(application_cursor, b'D'),
            ffi::UiTerminalKey::Home => b"\x1b[H".to_vec(),
            ffi::UiTerminalKey::End => b"\x1b[F".to_vec(),
            ffi::UiTerminalKey::Insert => b"\x1b[2~".to_vec(),
            ffi::UiTerminalKey::Delete => b"\x1b[3~".to_vec(),
            ffi::UiTerminalKey::PageUp => b"\x1b[5~".to_vec(),
            ffi::UiTerminalKey::PageDown => b"\x1b[6~".to_vec(),
            _ => Vec::new(),
        };
        if alt && !bytes.is_empty() {
            bytes.insert(0, 0x1b);
        }
        bytes
    }

    pub fn encode_paste(&self, port: ffi::UiSerialPort, text: &str) -> Vec<u8> {
        let normalized = text.replace("\r\n", "\n").replace('\n', "\r");
        if self.parser(port).screen().bracketed_paste() {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(normalized.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            bytes
        } else {
            normalized.into_bytes()
        }
    }
}

pub fn new_terminal_model() -> Box<TerminalModel> {
    Box::new(TerminalModel {
        parsers: [new_parser(), new_parser()],
    })
}

fn new_parser() -> Parser {
    vt100::Parser::new_with_callbacks(
        TERMINAL_ROWS,
        TERMINAL_COLUMNS,
        TERMINAL_SCROLLBACK_LINES,
        TerminalCallbacks::default(),
    )
}

fn port_index(port: ffi::UiSerialPort) -> usize {
    match port {
        ffi::UiSerialPort::Serial1 => 0,
        ffi::UiSerialPort::Serial2 => 1,
        _ => 0,
    }
}

fn cursor_sequence(application: bool, final_byte: u8) -> Vec<u8> {
    vec![0x1b, if application { b'O' } else { b'[' }, final_byte]
}

fn encode_text(text: &str, control: bool) -> Vec<u8> {
    if !control {
        return text.as_bytes().to_vec();
    }
    let Some(byte) = text.as_bytes().first().copied() else {
        return Vec::new();
    };
    let control_byte = match byte {
        b'@' | b' ' => 0,
        b'a'..=b'z' => byte - b'a' + 1,
        b'A'..=b'Z' => byte - b'A' + 1,
        b'['..=b'_' => byte - b'@',
        _ => return Vec::new(),
    };
    vec![control_byte]
}

fn cell_from_vt100(cell: &vt100::Cell) -> ffi::UiTerminalCell {
    let (foreground_kind, foreground_index, foreground_rgb) = color_from_vt100(cell.fgcolor());
    let (background_kind, background_index, background_rgb) = color_from_vt100(cell.bgcolor());
    ffi::UiTerminalCell {
        text: cell.contents().to_owned(),
        foreground_kind,
        foreground_index,
        foreground_red: foreground_rgb.0,
        foreground_green: foreground_rgb.1,
        foreground_blue: foreground_rgb.2,
        background_kind,
        background_index,
        background_red: background_rgb.0,
        background_green: background_rgb.1,
        background_blue: background_rgb.2,
        bold: cell.bold(),
        dim: cell.dim(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
        wide: cell.is_wide(),
        wide_continuation: cell.is_wide_continuation(),
    }
}

fn empty_cell() -> ffi::UiTerminalCell {
    ffi::UiTerminalCell {
        text: String::new(),
        foreground_kind: ffi::UiTerminalColorKind::Default,
        foreground_index: 0,
        foreground_red: 0,
        foreground_green: 0,
        foreground_blue: 0,
        background_kind: ffi::UiTerminalColorKind::Default,
        background_index: 0,
        background_red: 0,
        background_green: 0,
        background_blue: 0,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
        wide: false,
        wide_continuation: false,
    }
}

fn color_from_vt100(color: vt100::Color) -> (ffi::UiTerminalColorKind, u8, (u8, u8, u8)) {
    match color {
        vt100::Color::Default => (ffi::UiTerminalColorKind::Default, 0, (0, 0, 0)),
        vt100::Color::Idx(index) => (ffi::UiTerminalColorKind::Indexed, index, (0, 0, 0)),
        vt100::Color::Rgb(red, green, blue) => {
            (ffi::UiTerminalColorKind::Rgb, 0, (red, green, blue))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_keep_independent_vt100_state() {
        let mut model = new_terminal_model();
        model.process_output(ffi::UiSerialPort::Serial1, b"one\x1b[31m!");
        model.process_output(ffi::UiSerialPort::Serial2, b"two");
        let first = model.snapshot(ffi::UiSerialPort::Serial1);
        let second = model.snapshot(ffi::UiSerialPort::Serial2);
        assert_eq!(first.cells[0].text, "o");
        assert_eq!(first.cells[3].foreground_index, 1);
        assert_eq!(second.cells[0].text, "t");
        assert!(second.cells[3].text.is_empty());
    }

    #[test]
    fn input_encoder_tracks_cursor_and_bracketed_paste_modes() {
        let mut model = new_terminal_model();
        assert_eq!(
            model.encode_key(
                ffi::UiSerialPort::Serial1,
                ffi::UiTerminalKey::Up,
                "",
                false,
                false
            ),
            b"\x1b[A"
        );
        model.process_output(ffi::UiSerialPort::Serial1, b"\x1b[?1h\x1b[?2004h");
        assert_eq!(
            model.encode_key(
                ffi::UiSerialPort::Serial1,
                ffi::UiTerminalKey::Up,
                "",
                false,
                false
            ),
            b"\x1bOA"
        );
        assert_eq!(
            model.encode_paste(ffi::UiSerialPort::Serial1, "a\nb"),
            b"\x1b[200~a\rb\x1b[201~"
        );
    }

    #[test]
    fn scrollback_and_bell_are_reported() {
        let mut model = new_terminal_model();
        for _ in 0..30 {
            model.process_output(ffi::UiSerialPort::Serial1, b"line\r\n");
        }
        model.process_output(ffi::UiSerialPort::Serial1, b"\x07");
        model.set_scrollback(ffi::UiSerialPort::Serial1, usize::MAX);
        let snapshot = model.snapshot(ffi::UiSerialPort::Serial1);
        assert!(snapshot.scrollback > 0);
        assert_eq!(snapshot.bell_count, 1);
    }

    #[test]
    fn control_text_is_encoded_without_local_echo() {
        let model = new_terminal_model();
        assert_eq!(
            model.encode_key(
                ffi::UiSerialPort::Serial1,
                ffi::UiTerminalKey::Text,
                "c",
                true,
                false
            ),
            vec![3]
        );
    }
}
