use core::{cell::Cell, fmt};

// NOTE on current approach
const WHITE_MARK_BITS: u8 = 0b0000_0000;
const BLACK_MARK_BITS: u8 = 0b0000_0011;
const GREY_MARK_BITS: u8 = 0b0000_0001;

#[derive(Debug, Clone, Copy)]
pub struct HeaderFlags(pub(crate) u8);

impl HeaderFlags {
    pub const fn new_white() -> Self {
        Self(WHITE_MARK_BITS)
    }

    pub const fn new_black() -> Self {
        Self(BLACK_MARK_BITS)
    }

    pub const fn is_white(self) -> bool {
        // check only the color bits, ignoring IS_WEAK
        self.0 & BLACK_MARK_BITS == 0
    }

    pub const fn is_black(self) -> bool {
        self.0 & BLACK_MARK_BITS == BLACK_MARK_BITS
    }

    pub const fn is_grey(self) -> bool {
        self.0 & BLACK_MARK_BITS == GREY_MARK_BITS
    }

    pub fn mark_grey(self) -> Self {
        // set color bits to GREY (0b01) while preserving IS_WEAK
        // we must clear both color bits before ORing to prevent
        // silently turning weak-black (0b0011) into weak-grey (0b0011)
        Self((self.0 & !BLACK_MARK_BITS) | GREY_MARK_BITS)
    }

    pub const fn mark_black(self) -> Self {
        Self(self.0 | BLACK_MARK_BITS)
    }

    pub const fn mark_white(self) -> Self {
        // Clear the color bits while preserving IS_WEAK and any other flag bits
        Self(self.0 & !BLACK_MARK_BITS)
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

    pub const fn new_typed<const IS_WHITE: bool>() -> Self {
        // NOTE: We inverse the color when initializing the header. Because if the
        // target TraceColor is white, then the unmarked objects are white will be
        // made black
        const {
            if IS_WHITE {
                Self::new_black()
            } else {
                Self::new_white()
            }
        }
    }

    pub fn inc_roots(&self) {
        // crash on overflow to prevent memory bugs
        // having 65535 roots is practically impossible
        self.root_count.set(
            self.root_count
                .get()
                .checked_add(1)
                .expect("root count overflow: more than u16::MAX roots on a single GcBox"),
        );
    }

    pub fn dec_roots(&self) {
        // avoid crashing in a destructor if the root count somehow breaks
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
