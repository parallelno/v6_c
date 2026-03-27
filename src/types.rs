//! C type system for the v6c compiler targeting the Intel 8080.
//!
//! The 8080 is an 8-bit processor with 16-bit address space:
//! - `char`  = 1 byte  (8-bit)
//! - `int`   = 2 bytes (16-bit)
//! - `long`  = 4 bytes (32-bit)
//! - pointer = 2 bytes (16-bit address)

use std::fmt;

// ---------------------------------------------------------------------------
// CType – central type representation
// ---------------------------------------------------------------------------

/// Represents every C type the compiler can express.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CType {
    /// `void` – used as return type or pointer target.
    Void,

    /// `char` / `unsigned char` (1 byte).
    Char { signed: bool },

    /// `int` / `unsigned int` (2 bytes on 8080).
    Int { signed: bool },

    /// `long` / `unsigned long` (4 bytes).
    Long { signed: bool },

    /// Pointer to another type (2 bytes on 8080).
    Pointer(Box<CType>),

    /// Fixed-size array: element type + number of elements.
    Array {
        element: Box<CType>,
        size: usize,
    },

    /// Function type: return type + ordered parameter types.
    Function {
        return_type: Box<CType>,
        params: Vec<CType>,
    },

    /// `struct` type with named fields (no padding on 8080).
    Struct {
        /// Tag name (empty string for anonymous structs).
        tag: String,
        /// Ordered fields: `(field_name, field_type)`.
        members: Vec<(String, CType)>,
    },

    /// `union` type — all fields share the same starting offset.
    Union {
        /// Tag name (empty string for anonymous unions).
        tag: String,
        /// Fields: `(field_name, field_type)`.
        members: Vec<(String, CType)>,
    },

    /// `enum` type — syntactic sugar over `int`.
    Enum {
        /// Tag name (empty string for anonymous enums).
        tag: String,
    },

    /// `float` — 32-bit IEEE 754 single precision (4 bytes).
    /// Implemented via software floating-point library on the 8080.
    Float,
}

// ---------------------------------------------------------------------------
// StorageClass
// ---------------------------------------------------------------------------

/// C storage-class specifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageClass {
    Auto,
    Static,
    Extern,
    Register,
}

// ---------------------------------------------------------------------------
// CType – core query helpers
// ---------------------------------------------------------------------------

impl CType {
    /// Size in bytes on the 8080.
    ///
    /// Returns `None` for `Void` and `Function` which have no meaningful size.
    pub fn size_of(&self) -> Option<usize> {
        match self {
            CType::Void => None,
            CType::Char { .. } => Some(1),
            CType::Int { .. } => Some(2),
            CType::Long { .. } => Some(4),
            CType::Pointer(_) => Some(2),
            CType::Array { element, size } => element.size_of().map(|es| es * size),
            CType::Function { .. } => None,
            CType::Struct { members, .. } => {
                let mut total = 0usize;
                for (_, ty) in members {
                    total += ty.size_of()?;
                }
                Some(total)
            }
            CType::Union { members, .. } => {
                let mut max = 0usize;
                for (_, ty) in members {
                    let s = ty.size_of()?;
                    if s > max {
                        max = s;
                    }
                }
                Some(max)
            }
            CType::Enum { .. } => Some(2), // enum is int-sized
            CType::Float => Some(4),
        }
    }

    /// `true` for `Char`, `Int`, `Long`, and `Enum` (signed or unsigned).
    pub fn is_integer(&self) -> bool {
        matches!(self, CType::Char { .. } | CType::Int { .. } | CType::Long { .. } | CType::Enum { .. })
    }

    /// `true` for any `Pointer` type.
    pub fn is_pointer(&self) -> bool {
        matches!(self, CType::Pointer(_))
    }

    /// `true` when the type carries a sign (signed integer types).
    /// Non-integer types return `false`.
    pub fn is_signed(&self) -> bool {
        matches!(
            self,
            CType::Char { signed: true }
                | CType::Int { signed: true }
                | CType::Long { signed: true }
        )
    }

    /// `true` for `float`.
    pub fn is_float(&self) -> bool {
        matches!(self, CType::Float)
    }

    /// `true` for any arithmetic type (integer or floating-point).
    pub fn is_arithmetic(&self) -> bool {
        self.is_integer() || self.is_float()
    }

    /// `true` for types that can appear in most value contexts.
    pub fn is_scalar(&self) -> bool {
        self.is_arithmetic() || self.is_pointer()
    }

    /// `true` for `Void`.
    pub fn is_void(&self) -> bool {
        matches!(self, CType::Void)
    }

    /// `true` for `Array { .. }`.
    pub fn is_array(&self) -> bool {
        matches!(self, CType::Array { .. })
    }

    /// `true` for `Function { .. }`.
    pub fn is_function(&self) -> bool {
        matches!(self, CType::Function { .. })
    }

    /// `true` for `Struct { .. }`.
    pub fn is_struct(&self) -> bool {
        matches!(self, CType::Struct { .. })
    }

    /// `true` for `Union { .. }`.
    pub fn is_union(&self) -> bool {
        matches!(self, CType::Union { .. })
    }

    /// `true` for `Enum { .. }`.
    pub fn is_enum(&self) -> bool {
        matches!(self, CType::Enum { .. })
    }

    /// `true` for `Struct` or `Union`.
    pub fn is_struct_or_union(&self) -> bool {
        self.is_struct() || self.is_union()
    }

    /// Look up a field in a struct or union.
    ///
    /// Returns `(byte_offset, field_type)` if found.  For unions the offset
    /// is always 0.
    pub fn field_offset(&self, name: &str) -> Option<(usize, CType)> {
        match self {
            CType::Struct { members, .. } => {
                let mut offset = 0usize;
                for (fname, fty) in members {
                    if fname == name {
                        return Some((offset, fty.clone()));
                    }
                    offset += fty.size_of().unwrap_or(0);
                }
                None
            }
            CType::Union { members, .. } => {
                for (fname, fty) in members {
                    if fname == name {
                        return Some((0, fty.clone()));
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Integer conversion rank used for the usual arithmetic conversions.
    /// Higher rank wins during type promotion.
    ///
    /// Returns `None` for non-integer types.
    fn integer_rank(&self) -> Option<u8> {
        match self {
            CType::Char { .. } => Some(1),
            CType::Int { .. } | CType::Enum { .. } => Some(2),
            CType::Long { .. } => Some(3),
            _ => None,
        }
    }

    /// Return the pointee type if this is a `Pointer`, otherwise `None`.
    pub fn pointee(&self) -> Option<&CType> {
        match self {
            CType::Pointer(inner) => Some(inner),
            _ => None,
        }
    }

    /// Return the element type if this is an `Array`, otherwise `None`.
    pub fn element_type(&self) -> Option<&CType> {
        match self {
            CType::Array { element, .. } => Some(element),
            _ => None,
        }
    }

    /// Arrays decay to pointers to their element type.
    pub fn decay(&self) -> CType {
        match self {
            CType::Array { element, .. } => CType::Pointer(element.clone()),
            other => other.clone(),
        }
    }

    // --------------------------------------------------------------------
    // Implicit conversion rules
    // --------------------------------------------------------------------

    /// Can `self` be implicitly converted to `target`?
    ///
    /// Allowed implicit conversions (C89-style, no floats):
    /// 1. Identity (same type).
    /// 2. Integer widening: lower rank → higher rank (e.g. char → int → long).
    /// 3. Signed ↔ unsigned of the **same** width.
    /// 4. Any pointer ↔ `void *`.
    /// 5. Null pointer constant (integer 0) – handled at the call site, not here.
    /// 6. Array → pointer to element (decay).
    pub fn can_implicit_cast_to(&self, target: &CType) -> bool {
        // 1. Identity
        if self == target {
            return true;
        }

        let from = self.decay();
        let to = target.decay();

        if from == to {
            return true;
        }

        // 2 & 3. Integer conversions (widening or same-width sign change)
        if let (Some(from_rank), Some(to_rank)) = (from.integer_rank(), to.integer_rank()) {
            // Allow widening (lower rank → higher rank) or same-rank sign change
            return from_rank <= to_rank;
        }

        // Float ↔ integer conversions
        if (from.is_float() && to.is_integer()) || (from.is_integer() && to.is_float()) {
            return true;
        }

        // 4. Pointer ↔ void*
        if let (CType::Pointer(_), CType::Pointer(ref to_inner)) = (&from, &to) {
            if **to_inner == CType::Void {
                return true;
            }
        }
        if let (CType::Pointer(ref from_inner), CType::Pointer(_)) = (&from, &to) {
            if **from_inner == CType::Void {
                return true;
            }
        }

        false
    }
}

// ---------------------------------------------------------------------------
// Usual arithmetic conversions (§6.3.1.8 in C99)
// ---------------------------------------------------------------------------

/// Determine the common type for a binary arithmetic operation.
///
/// Implements the "usual arithmetic conversions" for integer types:
/// 1. Both operands are promoted to at least `int` width (integer promotion).
/// 2. If both have the same rank, the unsigned variant wins.
/// 3. Otherwise the wider type wins; if the wider type is signed and can
///    represent all values of the narrower unsigned type, the signed type
///    is used – on the 8080 this is always true for widening.
///
/// Returns `None` if either operand is not an integer type.
pub fn common_type(a: &CType, b: &CType) -> Option<CType> {
    // If either operand is float, the result is float.
    if a.is_float() || b.is_float() {
        return Some(CType::Float);
    }

    let a = integer_promote(a)?;
    let b = integer_promote(b)?;

    let rank_a = a.integer_rank().unwrap();
    let rank_b = b.integer_rank().unwrap();

    if rank_a == rank_b {
        // Same rank: if either is unsigned, result is unsigned.
        let signed = a.is_signed() && b.is_signed();
        return Some(match rank_a {
            2 => CType::Int { signed },
            3 => CType::Long { signed },
            _ => unreachable!("integer_promote guarantees rank >= 2"),
        });
    }

    // Different ranks – pick the higher-ranked type.
    // If the higher-ranked type is signed and the lower-ranked is unsigned,
    // the signed type is still chosen because on the 8080 each wider signed
    // type can represent every value of the narrower unsigned type.
    if rank_a > rank_b {
        Some(a)
    } else {
        Some(b)
    }
}

/// Integer promotion: types narrower than `int` are promoted to `int`.
///
/// `char` (signed or unsigned) → `int { signed: true }`.
/// `int` and `long` are returned as-is.
///
/// Returns `None` for non-integer types.
fn integer_promote(ty: &CType) -> Option<CType> {
    match ty {
        // char (signed or unsigned) promotes to signed int because
        // signed int can represent the full range of both on 8080.
        CType::Char { .. } => Some(CType::Int { signed: true }),
        CType::Int { .. } | CType::Long { .. } => Some(ty.clone()),
        // Enum promotes to int.
        CType::Enum { .. } => Some(CType::Int { signed: true }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

impl CType {
    pub fn char_signed() -> Self {
        CType::Char { signed: true }
    }
    pub fn char_unsigned() -> Self {
        CType::Char { signed: false }
    }
    pub fn int_signed() -> Self {
        CType::Int { signed: true }
    }
    pub fn int_unsigned() -> Self {
        CType::Int { signed: false }
    }
    pub fn long_signed() -> Self {
        CType::Long { signed: true }
    }
    pub fn long_unsigned() -> Self {
        CType::Long { signed: false }
    }
    pub fn ptr(inner: CType) -> Self {
        CType::Pointer(Box::new(inner))
    }
    pub fn void_ptr() -> Self {
        CType::Pointer(Box::new(CType::Void))
    }
    pub fn float() -> Self {
        CType::Float
    }
    pub fn array(element: CType, size: usize) -> Self {
        CType::Array {
            element: Box::new(element),
            size,
        }
    }
    pub fn function(return_type: CType, params: Vec<CType>) -> Self {
        CType::Function {
            return_type: Box::new(return_type),
            params,
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for CType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CType::Void => write!(f, "void"),
            CType::Char { signed: true } => write!(f, "char"),
            CType::Char { signed: false } => write!(f, "unsigned char"),
            CType::Int { signed: true } => write!(f, "int"),
            CType::Int { signed: false } => write!(f, "unsigned int"),
            CType::Long { signed: true } => write!(f, "long"),
            CType::Long { signed: false } => write!(f, "unsigned long"),
            CType::Pointer(inner) => write!(f, "{} *", inner),
            CType::Array { element, size } => write!(f, "{}[{}]", element, size),
            CType::Function {
                return_type,
                params,
            } => {
                write!(f, "{}(", return_type)?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ")")
            }
            CType::Struct { tag, .. } => {
                if tag.is_empty() {
                    write!(f, "struct <anonymous>")
                } else {
                    write!(f, "struct {}", tag)
                }
            }
            CType::Union { tag, .. } => {
                if tag.is_empty() {
                    write!(f, "union <anonymous>")
                } else {
                    write!(f, "union {}", tag)
                }
            }
            CType::Enum { tag } => {
                if tag.is_empty() {
                    write!(f, "enum <anonymous>")
                } else {
                    write!(f, "enum {}", tag)
                }
            }
            CType::Float => write!(f, "float"),
        }
    }
}

impl fmt::Debug for CType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Reuse Display for Debug – the C-style syntax is clear enough.
        write!(f, "CType({})", self)
    }
}

impl fmt::Display for StorageClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageClass::Auto => write!(f, "auto"),
            StorageClass::Static => write!(f, "static"),
            StorageClass::Extern => write!(f, "extern"),
            StorageClass::Register => write!(f, "register"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- size_of ----------------------------------------------------------

    #[test]
    fn size_of_primitives() {
        assert_eq!(CType::Void.size_of(), None);
        assert_eq!(CType::char_signed().size_of(), Some(1));
        assert_eq!(CType::char_unsigned().size_of(), Some(1));
        assert_eq!(CType::int_signed().size_of(), Some(2));
        assert_eq!(CType::int_unsigned().size_of(), Some(2));
        assert_eq!(CType::long_signed().size_of(), Some(4));
        assert_eq!(CType::long_unsigned().size_of(), Some(4));
    }

    #[test]
    fn size_of_pointer() {
        let p = CType::ptr(CType::int_signed());
        assert_eq!(p.size_of(), Some(2));

        let pp = CType::ptr(CType::ptr(CType::char_signed()));
        assert_eq!(pp.size_of(), Some(2));
    }

    #[test]
    fn size_of_array() {
        let arr = CType::array(CType::int_signed(), 10);
        assert_eq!(arr.size_of(), Some(20));

        let arr_long = CType::array(CType::long_signed(), 5);
        assert_eq!(arr_long.size_of(), Some(20));

        let arr_void = CType::array(CType::Void, 3);
        assert_eq!(arr_void.size_of(), None);
    }

    #[test]
    fn size_of_function() {
        let ft = CType::function(CType::int_signed(), vec![CType::char_signed()]);
        assert_eq!(ft.size_of(), None);
    }

    // -- classification ---------------------------------------------------

    #[test]
    fn is_integer_positive() {
        assert!(CType::char_signed().is_integer());
        assert!(CType::int_unsigned().is_integer());
        assert!(CType::long_signed().is_integer());
    }

    #[test]
    fn is_integer_negative() {
        assert!(!CType::Void.is_integer());
        assert!(!CType::ptr(CType::int_signed()).is_integer());
        assert!(!CType::array(CType::char_signed(), 5).is_integer());
    }

    #[test]
    fn is_pointer_checks() {
        assert!(CType::ptr(CType::Void).is_pointer());
        assert!(!CType::int_signed().is_pointer());
    }

    #[test]
    fn is_signed_checks() {
        assert!(CType::char_signed().is_signed());
        assert!(!CType::char_unsigned().is_signed());
        assert!(CType::int_signed().is_signed());
        assert!(!CType::int_unsigned().is_signed());
        assert!(CType::long_signed().is_signed());
        assert!(!CType::long_unsigned().is_signed());
        assert!(!CType::Void.is_signed());
    }

    #[test]
    fn is_scalar_checks() {
        assert!(CType::int_signed().is_scalar());
        assert!(CType::ptr(CType::char_signed()).is_scalar());
        assert!(!CType::Void.is_scalar());
        assert!(!CType::array(CType::int_signed(), 4).is_scalar());
    }

    // -- decay ------------------------------------------------------------

    #[test]
    fn array_decays_to_pointer() {
        let arr = CType::array(CType::int_signed(), 10);
        assert_eq!(arr.decay(), CType::ptr(CType::int_signed()));
    }

    #[test]
    fn non_array_decay_is_identity() {
        let i = CType::int_signed();
        assert_eq!(i.decay(), i);
    }

    // -- pointee / element_type -------------------------------------------

    #[test]
    fn pointee_works() {
        let p = CType::ptr(CType::long_signed());
        assert_eq!(p.pointee(), Some(&CType::long_signed()));
        assert_eq!(CType::int_signed().pointee(), None);
    }

    #[test]
    fn element_type_works() {
        let a = CType::array(CType::char_unsigned(), 8);
        assert_eq!(a.element_type(), Some(&CType::char_unsigned()));
        assert_eq!(CType::int_signed().element_type(), None);
    }

    // -- implicit cast ----------------------------------------------------

    #[test]
    fn identity_cast() {
        let t = CType::int_signed();
        assert!(t.can_implicit_cast_to(&t));
    }

    #[test]
    fn widening_casts() {
        assert!(CType::char_signed().can_implicit_cast_to(&CType::int_signed()));
        assert!(CType::char_signed().can_implicit_cast_to(&CType::long_signed()));
        assert!(CType::int_signed().can_implicit_cast_to(&CType::long_signed()));
        assert!(CType::char_unsigned().can_implicit_cast_to(&CType::int_unsigned()));
    }

    #[test]
    fn same_width_sign_change() {
        assert!(CType::int_signed().can_implicit_cast_to(&CType::int_unsigned()));
        assert!(CType::int_unsigned().can_implicit_cast_to(&CType::int_signed()));
    }

    #[test]
    fn narrowing_rejected() {
        assert!(!CType::int_signed().can_implicit_cast_to(&CType::char_signed()));
        assert!(!CType::long_signed().can_implicit_cast_to(&CType::int_signed()));
    }

    #[test]
    fn pointer_to_void_ptr() {
        let int_ptr = CType::ptr(CType::int_signed());
        let void_ptr = CType::void_ptr();
        assert!(int_ptr.can_implicit_cast_to(&void_ptr));
        assert!(void_ptr.can_implicit_cast_to(&int_ptr));
    }

    #[test]
    fn incompatible_pointer_types() {
        let int_ptr = CType::ptr(CType::int_signed());
        let char_ptr = CType::ptr(CType::char_signed());
        assert!(!int_ptr.can_implicit_cast_to(&char_ptr));
    }

    #[test]
    fn array_to_pointer_cast() {
        let arr = CType::array(CType::int_signed(), 5);
        let ptr = CType::ptr(CType::int_signed());
        assert!(arr.can_implicit_cast_to(&ptr));
    }

    // -- common_type (usual arithmetic conversions) -----------------------

    #[test]
    fn common_type_same() {
        assert_eq!(
            common_type(&CType::int_signed(), &CType::int_signed()),
            Some(CType::int_signed())
        );
    }

    #[test]
    fn common_type_char_promoted_to_int() {
        assert_eq!(
            common_type(&CType::char_signed(), &CType::char_unsigned()),
            Some(CType::int_signed())
        );
    }

    #[test]
    fn common_type_int_and_long() {
        assert_eq!(
            common_type(&CType::int_signed(), &CType::long_signed()),
            Some(CType::long_signed())
        );
    }

    #[test]
    fn common_type_signed_unsigned_same_rank() {
        assert_eq!(
            common_type(&CType::int_signed(), &CType::int_unsigned()),
            Some(CType::int_unsigned())
        );
    }

    #[test]
    fn common_type_unsigned_int_and_signed_long() {
        // Signed long can represent all unsigned int values on 8080, so long wins.
        assert_eq!(
            common_type(&CType::int_unsigned(), &CType::long_signed()),
            Some(CType::long_signed())
        );
    }

    #[test]
    fn common_type_non_integer_returns_none() {
        assert_eq!(common_type(&CType::Void, &CType::int_signed()), None);
        assert_eq!(
            common_type(&CType::ptr(CType::int_signed()), &CType::int_signed()),
            None
        );
    }

    // -- display ----------------------------------------------------------

    #[test]
    fn display_primitives() {
        assert_eq!(format!("{}", CType::Void), "void");
        assert_eq!(format!("{}", CType::char_signed()), "char");
        assert_eq!(format!("{}", CType::char_unsigned()), "unsigned char");
        assert_eq!(format!("{}", CType::int_signed()), "int");
        assert_eq!(format!("{}", CType::int_unsigned()), "unsigned int");
        assert_eq!(format!("{}", CType::long_signed()), "long");
        assert_eq!(format!("{}", CType::long_unsigned()), "unsigned long");
    }

    #[test]
    fn display_pointer() {
        assert_eq!(format!("{}", CType::ptr(CType::int_signed())), "int *");
        assert_eq!(format!("{}", CType::void_ptr()), "void *");
    }

    #[test]
    fn display_array() {
        assert_eq!(
            format!("{}", CType::array(CType::char_signed(), 16)),
            "char[16]"
        );
    }

    #[test]
    fn display_function() {
        let ft = CType::function(
            CType::int_signed(),
            vec![CType::char_signed(), CType::ptr(CType::char_signed())],
        );
        assert_eq!(format!("{}", ft), "int(char, char *)");
    }

    #[test]
    fn debug_uses_display() {
        let t = CType::int_signed();
        assert_eq!(format!("{:?}", t), "CType(int)");
    }
}
