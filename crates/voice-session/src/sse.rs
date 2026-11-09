/// Framing without accumulating a line or an event. JSON receives data bytes
/// directly; arbitrary comments and redundant terminal snapshots use no buffer.
#[derive(Default)]
pub struct Framer {
    field: [u8; 5],
    used: usize,
    mode: u8,
    blank: bool,
    cr: bool,
    data: bool,
}
#[derive(Clone, Copy)]
pub enum Token {
    Byte(u8),
    End,
}
impl Framer {
    pub fn push(&mut self, b: u8) -> Option<Token> {
        if self.cr {
            self.cr = false;
            if b == b'\n' {
                return None;
            }
        }
        if matches!(b, b'\n' | b'\r') {
            self.cr = b == b'\r';
            let empty = self.blank || (self.used == 0 && self.mode == 0);
            let bare = self.mode == 0 && self.used == 4 && &self.field[..4] == b"data";
            self.used = 0;
            self.mode = 0;
            self.blank = true;
            if empty && self.data {
                self.data = false;
                return Some(Token::End);
            }
            if bare {
                let previous = self.data;
                self.data = true;
                if previous {
                    return Some(Token::Byte(b'\n'));
                }
            }
            return None;
        }
        self.blank = false;
        match self.mode {
            0 => {
                if b == b':' {
                    if self.used == 4 && &self.field[..4] == b"data" {
                        self.mode = 1;
                        let previous = self.data;
                        self.data = true;
                        if previous {
                            return Some(Token::Byte(b'\n'));
                        }
                    } else {
                        self.mode = 3;
                    }
                } else {
                    if self.used < 5 {
                        self.field[self.used] = b;
                    }
                    self.used = self.used.saturating_add(1).min(6);
                }
            }
            1 => {
                self.mode = 2;
                if b != b' ' {
                    return Some(Token::Byte(b));
                }
            }
            2 => return Some(Token::Byte(b)),
            _ => (),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    #[test]
    fn comments_crlf_and_multiple_data_lines_do_not_need_an_event_buffer() {
        let mut framing = Framer::default();
        let mut events = Vec::new();
        let mut data = Vec::new();
        let wire =
            b":keepalive\r\nevent: ignored\r\ndata: first\r\ndata: second\r\n\r\ndata: third\r\r";
        for b in wire {
            match framing.push(*b) {
                Some(Token::Byte(b)) => data.push(b),
                Some(Token::End) => events.push(core::mem::take(&mut data)),
                None => (),
            }
        }
        assert_eq!(events, [b"first\nsecond".to_vec(), b"third".to_vec()]);
        for b in core::iter::once(b':')
            .chain(core::iter::repeat_n(b'x', 500_000))
            .chain(core::iter::once(b'\n'))
        {
            assert!(framing.push(b).is_none());
        }
    }
}
