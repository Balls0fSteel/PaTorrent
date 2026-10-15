//! A tiny replacement for libtorrent's `flags::bitfield_flag<>` (and the `bitflags` crate).

/// Declares a `Copy` bit-set newtype with named constants and the usual set operators.
macro_rules! bit_flags {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident: $repr:ty {
            $( $(#[$fmeta:meta])* const $flag:ident = $bit:expr; )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        $vis struct $name($repr);

        #[allow(dead_code)]
        impl $name {
            $( $(#[$fmeta])* pub const $flag: $name = $name(1 << $bit); )*

            pub const fn empty() -> Self { Self(0) }
            pub const fn all() -> Self { Self(0 $(| (1 << $bit))*) }
            pub const fn bits(self) -> $repr { self.0 }
            pub const fn from_bits_truncate(bits: $repr) -> Self { Self(bits & Self::all().0) }
            pub const fn is_empty(self) -> bool { self.0 == 0 }
            pub const fn contains(self, other: Self) -> bool { self.0 & other.0 == other.0 }
            pub const fn intersects(self, other: Self) -> bool { self.0 & other.0 != 0 }
            pub fn insert(&mut self, other: Self) { self.0 |= other.0 }
            pub fn remove(&mut self, other: Self) { self.0 &= !other.0 }
            pub fn set(&mut self, other: Self, on: bool) { if on { self.insert(other) } else { self.remove(other) } }
        }

        impl std::ops::BitOr for $name { type Output = Self; fn bitor(self, o: Self) -> Self { Self(self.0 | o.0) } }
        impl std::ops::BitAnd for $name { type Output = Self; fn bitand(self, o: Self) -> Self { Self(self.0 & o.0) } }
        impl std::ops::Not for $name { type Output = Self; fn not(self) -> Self { Self(!self.0 & Self::all().0) } }
        impl std::ops::BitOrAssign for $name { fn bitor_assign(&mut self, o: Self) { self.0 |= o.0 } }
        impl std::ops::BitAndAssign for $name { fn bitand_assign(&mut self, o: Self) { self.0 &= o.0 } }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let mut first = true;
                f.write_str(concat!(stringify!($name), "("))?;
                $(
                    if self.contains(Self::$flag) {
                        if !first { f.write_str(" | ")?; }
                        f.write_str(stringify!($flag))?;
                        first = false;
                    }
                )*
                let _ = first;
                f.write_str(")")
            }
        }
    };
}

pub(crate) use bit_flags;

#[cfg(test)]
mod tests {
    bit_flags! {
        struct Demo: u8 {
            const A = 0;
            const B = 1;
            const C = 7;
        }
    }

    #[test]
    fn set_operations() {
        let mut f = Demo::A | Demo::C;
        assert!(f.contains(Demo::A) && !f.contains(Demo::B));
        assert!(f.intersects(Demo::C | Demo::B));
        f.remove(Demo::A);
        f.set(Demo::B, true);
        assert_eq!(f, Demo::B | Demo::C);
        assert_eq!(!Demo::A, Demo::B | Demo::C);
        assert_eq!(Demo::from_bits_truncate(0xff), Demo::all());
        assert_eq!(format!("{:?}", Demo::A | Demo::C), "Demo(A | C)");
        assert_eq!(format!("{:?}", Demo::empty()), "Demo()");
    }
}
