use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkStrategy {
    Symbol = 0,
    Recursive = 1,
}

impl From<u8> for ChunkStrategy {
    #[inline]
    fn from(val: u8) -> Self {
        match val {
            1 => Self::Recursive,
            _ => Self::Symbol,
        }
    }
}
