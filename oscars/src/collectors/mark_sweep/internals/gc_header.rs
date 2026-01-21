use core::{cell::Cell, fmt};

// NOTE on current approach
const WHITE_MARK_BITS: u8 = 0b0000_0000;
const BLACK_MARK_BITS: u8 = 0b0000_0011;
const GREY_MARK_BITS: u8 = 0b0000_0001;

// Whether the box is weak or not;
const IS_WEAK: u8 = 0b0001_0000;

#[derive(Debug, Clone, Copy)]
pub struct HeaderFlags(pub(crate) u8);

impl HeaderFlags {
    pub const fn new_white() -> Self {
        Self(WHITE_MARK_BITS)
    }

    pub const fn new_black() -> Self {
        Self(BLACK_MARK_BITS)
    }

    pub const fn weak_white() -> Self {
        Self(WHITE_MARK_BITS | IS_WEAK)
    }

    pub const fn weak_black() -> Self {
        Self(BLACK_MARK_BITS | IS_WEAK)
    }

    pub const fn is_weak(self) -> bool {
        self.0 & IS_WEAK == IS_WEAK
    }

    pub const fn is_white(self) -> bool {
        self.0 | WHITE_MARK_BITS == WHITE_MARK_BITS
    }

    pub const fn is_black(self) -> bool {
        self.0 & BLACK_MARK_BITS == BLACK_MARK_BITS
    }

    pub const fn is_grey(self) -> bool {
        self.0 & BLACK_MARK_BITS == GREY_MARK_BITS
    }

    pub fn mark_grey(self) -> Self {
        if self.is_white() {
            Self(self.0 | GREY_MARK_BITS)
        } else {
            Self(self.0 & GREY_MARK_BITS)
        }
    }

    pub const fn mark_black(self) -> Self {
        Self(self.0 | BLACK_MARK_BITS)
    }

    pub const fn mark_white(self) -> Self {
        Self((self.0 | BLACK_MARK_BITS) & WHITE_MARK_BITS)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum HeaderColor {
    White,
    Black,
    Grey,
}

pub struct GcHeader {
    pub(crate) flags: Cell<HeaderFlags>,
    root_count: Cell<u16>,
}

impl fmt::Debug for GcHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GcHeader {{ flags: {:08b}, roots: {} }} ",
            self.flags.get().0,
            self.root_count.get()
        )
    }
}

impl GcHeader {
    /// Creates a new `NeoGcHeader`.
    pub const fn new_white() -> Self {
        Self {
            flags: Cell::new(HeaderFlags::new_white()),
            root_count: Cell::new(0),
        }
    }

    pub const fn new_black() -> Self {
        Self {
            flags: Cell::new(HeaderFlags::new_black()),
            root_count: Cell::new(0),
        }
    }

    /// Create a `NeoGcHeader` that is flagged as weak.
    pub const fn weak_white() -> Self {
        Self {
            flags: Cell::new(HeaderFlags::weak_white()),
            root_count: Cell::new(0),
        }
    }

    pub const fn weak_black() -> Self {
        Self {
            flags: Cell::new(HeaderFlags::weak_black()),
            root_count: Cell::new(0),
        }
    }

    pub const fn new_typed<const IS_WHITE: bool, const IS_WEAK: bool>() -> Self {
        // NOTE: We inverse the color when initializing the header. Because if the
        // target TraceColor is white, then the unmarked objects are white will be
        // made black
        const {
            match (IS_WHITE, IS_WEAK) {
                (true, false) => Self::new_black(),
                (false, false) => Self::new_white(),
                (true, true) => Self::weak_black(),
                (false, true) => Self::weak_white(),
            }
        }
    }

    pub const fn is_weak(&self) -> bool {
        self.flags.get().is_weak()
    }

    pub fn inc_roots(&self) {
        // NOTE: This may panic or overflow after 2^16 - 1 roots
        self.root_count.set(self.root_count.get() + 1);
    }

    pub fn dec_roots(&self) {
        // NOTE: if we are underflowing on subtraction, something is seriously wrong
        // with the codebase.
        self.root_count.set(self.root_count.get().saturating_sub(1));
    }

    pub fn is_rooted(&self) -> bool {
        self.root_count.get() > 0
    }

    pub fn roots(&self) -> u16 {
        self.root_count.get()
    }

    pub fn mark(&self, color: HeaderColor) {
        match color {
            HeaderColor::White => self.flags.set(self.flags.get().mark_white()),
            HeaderColor::Black => self.flags.set(self.flags.get().mark_black()),
            HeaderColor::Grey => self.flags.set(self.flags.get().mark_grey()),
        }
    }

    pub const fn is_white(&self) -> bool {
        self.flags.get().is_white()
    }

    pub const fn is_black(&self) -> bool {
        self.flags.get().is_black()
    }

    pub const fn is_grey(&self) -> bool {
        self.flags.get().is_grey()
    }
}

#[cfg(test)]
mod tests {
    use super::{BLACK_MARK_BITS, GREY_MARK_BITS, WHITE_MARK_BITS};
    use super::{GcHeader, HeaderColor};
    #[test]
    fn header_marking() {
        let header = GcHeader::new_white();
        assert!(header.is_white());
        assert_eq!(header.flags.get().0, WHITE_MARK_BITS);
        assert!(!header.is_black());
        assert!(!header.is_grey());
        header.mark(HeaderColor::Grey);
        assert_eq!(header.flags.get().0, GREY_MARK_BITS);
        assert!(header.is_grey());
        assert!(!header.is_black());
        header.mark(HeaderColor::Black);
        assert_eq!(header.flags.get().0, BLACK_MARK_BITS);
        assert!(header.is_black());
        header.mark(HeaderColor::Grey);
        assert_eq!(header.flags.get().0, GREY_MARK_BITS);
        assert!(header.is_grey());
        header.mark(HeaderColor::White);
        assert_eq!(header.flags.get().0, WHITE_MARK_BITS);
        assert!(header.is_white());
        assert!(!header.is_black(), "failed to toggle white");
        assert!(!header.is_grey(), "failed to toggle white");
        header.mark(HeaderColor::Black);
        assert!(header.is_black());
        assert!(!header.is_white(), "failed to toggle black");
        assert!(!header.is_grey(), "failed to toggle black");
    }
}
