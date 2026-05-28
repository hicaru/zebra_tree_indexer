#[allow(dead_code)]
pub(crate) struct OutputPos {
    pub char_offset: usize,
    pub line: u32,
    pub column: u32,
}

pub(crate) struct BytePos {
    pub byte_offset: usize,
    pub output: Option<OutputPos>,
}

impl BytePos {
    pub fn new(byte_offset: usize) -> Self {
        Self { byte_offset, output: None }
    }
}

pub(crate) fn compute_positions(
    text: &str,
    mut positions: Vec<&mut BytePos>,
) {
    positions.sort_by_key(|p| p.byte_offset);

    let mut iter = positions.into_iter();
    let Some(mut next) = iter.next() else {
        return;
    };

    let mut char_off = 0usize;
    let mut line = 1u32;
    let mut col = 1u32;

    for (byte_off, ch) in text.char_indices() {
        while next.byte_offset == byte_off {
            next.output = Some(OutputPos { char_offset: char_off, line, column: col });
            match iter.next() {
                Some(p) => next = p,
                None => return,
            }
        }
        char_off += 1;
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    loop {
        next.output = Some(OutputPos { char_offset: char_off, line, column: col });
        match iter.next() {
            Some(p) => next = p,
            None => return,
        }
    }
}
