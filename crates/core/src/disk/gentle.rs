//! Stand-in until the platform calls land (D48 part 1): background I/O is a no-op.

pub(crate) struct BackgroundIo;

impl BackgroundIo {
    pub(crate) fn begin() -> BackgroundIo {
        BackgroundIo
    }

    pub(crate) fn active(&self) -> bool {
        false
    }
}

pub(crate) fn advise_sequential(_f: &std::fs::File) {}

pub(crate) fn advise_done(_f: &std::fs::File, _offset: u64, _len: u64) {}
