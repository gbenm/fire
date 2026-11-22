//! Turns raw terminal output into plain text agents can read: drops ANSI/OSC escape sequences
//! and replays carriage returns, backspaces and line erases so progress bars collapse to their
//! last state.

pub(crate) fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut line = Line::default();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut action = None;
                    for next in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&next) {
                            action = Some(next);
                            break;
                        }
                        params.push(next);
                    }
                    let count = params.parse::<usize>().unwrap_or(1).max(1);
                    match action {
                        Some('K') => line.erase(&params),
                        Some('G') => line.col = count - 1,
                        Some('C') => line.col += count,
                        Some('D') => line.col = line.col.saturating_sub(count),
                        _ => {}
                    }
                }
                // OSC, DCS, SOS, PM, APC: strings terminated by BEL or ST (ESC \).
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(next) = chars.next() {
                        if next == '\x07' {
                            break;
                        }
                        if next == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                // Character set designations take one more character.
                Some('(' | ')' | '*' | '+' | '#' | '%') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => line.col = 0,
            '\n' => {
                out.push_str(line.take().trim_end());
                out.push('\n');
            }
            '\x08' => line.col = line.col.saturating_sub(1),
            '\t' => line.put('\t'),
            ch if ch.is_control() => {}
            ch => line.put(ch),
        }
    }

    let rest = line.take();
    out.push_str(rest.trim_end());
    out
}

#[derive(Default)]
struct Line {
    chars: Vec<char>,
    col: usize,
}

impl Line {
    fn put(&mut self, ch: char) {
        if self.col < self.chars.len() {
            self.chars[self.col] = ch;
        } else {
            self.chars.resize(self.col, ' ');
            self.chars.push(ch);
        }
        self.col += 1;
    }

    fn erase(&mut self, mode: &str) {
        match mode {
            "1" => {
                let end = (self.col + 1).min(self.chars.len());
                self.chars[..end].fill(' ');
            }
            "2" => self.chars.clear(),
            _ => self.chars.truncate(self.col),
        }
    }

    fn take(&mut self) -> String {
        self.col = 0;
        std::mem::take(&mut self.chars).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn strips_colors_and_osc_titles() {
        let raw = "\x1b]0;my title\x07\x1b[1;32mready\x1b[0m in 120ms\r\n";
        assert_eq!(clean(raw), "ready in 120ms\n");
        assert_eq!(
            clean("\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\\n"),
            "link\n"
        );
    }

    #[test]
    fn collapses_carriage_return_progress() {
        let raw = "building  10%\rbuilding  55%\rbuilding 100%\r\ndone\r\n";
        assert_eq!(clean(raw), "building 100%\ndone\n");
    }

    #[test]
    fn shorter_rewrite_needs_erase_line() {
        assert_eq!(clean("long status\rok\n"), "okng status\n");
        assert_eq!(clean("long status\r\x1b[2Kok\n"), "ok\n");
        assert_eq!(clean("long status\r\x1b[Kok\n"), "ok\n");
    }

    #[test]
    fn handles_backspace_and_unterminated_tail() {
        assert_eq!(clean("abc\x08\x08X"), "aXc");
        assert_eq!(clean("prompt> "), "prompt>");
    }
}
