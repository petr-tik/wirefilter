use crate::{
    lex::{expect, skip_space, Lex, LexResult, LexWith},
    rhs_types::{Bytes, IpRange, UninhabitedBool, UninhabitedMap},
    scheme::FieldPathItem,
    strict_partial_ord::StrictPartialOrd,
};
use failure::Fail;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::HashMap,
    convert::TryFrom,
    fmt::{self, Debug, Formatter},
    net::IpAddr,
    ops::RangeInclusive,
};

fn lex_rhs_values<'i, T: Lex<'i>>(input: &'i str) -> LexResult<'i, Vec<T>> {
    let mut input = expect(input, "{")?;
    let mut res = Vec::new();
    loop {
        input = skip_space(input);
        if let Ok(rest) = expect(input, "}") {
            input = rest;
            return Ok((res, input));
        } else {
            let (item, rest) = T::lex(input)?;
            res.push(item);
            input = rest;
        }
    }
}

/// An error that occurs on a type mismatch.
#[derive(Debug, PartialEq, Fail)]
#[fail(
    display = "expected value of type {:?}, but got {:?}",
    expected, actual
)]
pub struct TypeMismatchError {
    /// Expected value type.
    pub expected: Type,
    /// Provided value type.
    pub actual: Type,
}

macro_rules! replace_underscore {
    ($name:ident ($val_ty:ty)) => {Type::$name(_)};
    ($name:ident) => {Type::$name};
}

macro_rules! specialized_get_type {
    (Map, $value:ident) => {
        Type::Map(Box::new($value.get_type()))
    };
    ($name:ident, $value:ident) => {
        Type::$name
    };
}

macro_rules! specialized_type_mismatch {
    (Map, $value:ident) => {
        unreachable!()
    };
    ($name:ident, $value:ident) => {
        Err(TypeMismatchError {
            expected: Type::$name,
            actual: $value.get_type(),
        })
    };
}

macro_rules! declare_types {
    ($(# $attrs:tt)* enum $name:ident $(<$lt:tt>)* { $($(# $vattrs:tt)* $variant:ident ( $ty:ty ) , )* }) => {
        $(# $attrs)*
        #[repr(u8)]
        pub enum $name $(<$lt>)* {
            $($(# $vattrs)* $variant($ty),)*
        }

        impl $(<$lt>)* GetType for $name $(<$lt>)* {
            fn get_type(&self) -> Type {
                match self {
                    $($name::$variant(_value) => specialized_get_type!($variant, _value),)*
                }
            }
        }

        impl $(<$lt>)* Debug for $name $(<$lt>)* {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                match self {
                    $($name::$variant(inner) => Debug::fmt(inner, f),)*
                }
            }
        }
    };

    ($($(# $attrs:tt)* $name:ident $([$val_ty:ty])? ( $(# $lhs_attrs:tt)* $lhs_ty:ty | $rhs_ty:ty | $multi_rhs_ty:ty ) , )*) => {
        /// Enumeration of supported types for field values.
        #[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
        #[repr(C)]
        pub enum Type {
            $($(# $attrs)* $name$(($val_ty))?,)*
        }

        impl Type {
            /// Returns the inner type when available (e.g: for a Map)
            pub fn next(&self) -> Option<Type> {
                match self {
                    Type::Map(ty) => Some(*ty.clone()),
                    _ => None,
                }
            }
        }

        /// Provides a way to get a [`Type`] of the implementor.
        pub trait GetType {
            /// Returns a type.
            fn get_type(&self) -> Type;
        }

        impl GetType for Type {
            fn get_type(&self) -> Type {
                self.clone()
            }
        }

        declare_types! {
            /// An LHS value provided for filter execution.
            ///
            /// These are passed to the [execution context](::ExecutionContext)
            /// and are used by [filters](::Filter)
            /// for execution and comparisons.
            #[derive(PartialEq, Eq, Clone, Deserialize)]
            #[serde(untagged)]
            enum LhsValue<'a> {
                $($(# $attrs)* $(# $lhs_attrs)* $name($lhs_ty),)*
            }
        }

        $(impl<'a> From<$lhs_ty> for LhsValue<'a> {
            fn from(value: $lhs_ty) -> Self {
                LhsValue::$name(value)
            }
        })*
        //Map<>::try_from(ip_lhs_value)
        $(impl<'a> TryFrom<LhsValue<'a>> for $lhs_ty {
            type Error = TypeMismatchError;

            fn try_from(value: LhsValue<'a>) -> Result<$lhs_ty, TypeMismatchError> {
                match value {
                    LhsValue::$name(value) => Ok(value),
                    _ => specialized_type_mismatch!($name, value),
                }
            }
        })*

        declare_types! {
            /// An RHS value parsed from a filter string.
            #[derive(PartialEq, Eq, Clone, Serialize)]
            #[serde(untagged)]
            enum RhsValue {
                $($(# $attrs)* $name($rhs_ty),)*
            }
        }

        impl<'i> LexWith<'i, Type> for RhsValue {
            fn lex_with(input: &str, ty: Type) -> LexResult<'_, Self> {
                Ok(match ty {
                    $(replace_underscore!($name $(($val_ty))?) => {
                        let (value, input) = <$rhs_ty>::lex(input)?;
                        (RhsValue::$name(value), input)
                    })*
                })
            }
        }

        impl<'a> PartialOrd<RhsValue> for LhsValue<'a> {
            fn partial_cmp(&self, other: &RhsValue) -> Option<Ordering> {
                match (self, other) {
                    $((LhsValue::$name(lhs), RhsValue::$name(rhs)) => {
                        lhs.strict_partial_cmp(rhs)
                    },)*
                    _ => None,
                }
            }
        }

        impl<'a> StrictPartialOrd<RhsValue> for LhsValue<'a> {}

        impl<'a> PartialEq<RhsValue> for LhsValue<'a> {
            fn eq(&self, other: &RhsValue) -> bool {
                self.strict_partial_cmp(other) == Some(Ordering::Equal)
            }
        }

        declare_types! {
            /// A typed group of a list of values.
            ///
            /// This is used for `field in { ... }` operation that allows
            /// only same-typed values in a list.
            #[derive(PartialEq, Eq, Clone, Serialize)]
            #[serde(untagged)]
            enum RhsValues {
                $($(# $attrs)* $name(Vec<$multi_rhs_ty>),)*
            }
        }

        impl<'i> LexWith<'i, Type> for RhsValues {
            fn lex_with(input: &str, ty: Type) -> LexResult<'_, Self> {
                Ok(match ty {
                    $(replace_underscore!($name $(($val_ty))?) => {
                        let (value, input) = lex_rhs_values(input)?;
                        (RhsValues::$name(value), input)
                    })*
                })
            }
        }
    };
}

// type Map<'a> = HashMap<&'a str, LhsValue<'a>>;
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Map<'a>(Type, #[serde(borrow)] HashMap<String, LhsValue<'a>>);

impl<'a> Map<'a> {
    pub fn new(ty: Type) -> Self {
        Self {
            0: ty,
            1: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&LhsValue<'a>> {
        self.1.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut LhsValue<'a>> {
        self.1.get_mut(key)
    }

    pub fn insert(
        &mut self,
        key: String,
        value: LhsValue<'a>,
    ) -> Result<Option<LhsValue<'a>>, TypeMismatchError> {
        let value_type = value.get_type();
        if self.0 != value_type {
            return Err(TypeMismatchError {
                expected: self.0.clone(),
                actual: value_type,
            });
        }
        Ok(self.1.insert(key, value))
    }
}

impl<'a> GetType for Map<'a> {
    fn get_type(&self) -> Type {
        self.0.clone()
    }
}

// special case for simply passing bytes
impl<'a> From<&'a [u8]> for LhsValue<'a> {
    fn from(b: &'a [u8]) -> Self {
        LhsValue::Bytes(Cow::Borrowed(b))
    }
}

// special case for simply passing strings
impl<'a> From<&'a str> for LhsValue<'a> {
    fn from(s: &'a str) -> Self {
        s.as_bytes().into()
    }
}

impl<'a> From<&'a RhsValue> for LhsValue<'a> {
    fn from(rhs_value: &'a RhsValue) -> Self {
        match rhs_value {
            RhsValue::Ip(ip) => LhsValue::Ip(*ip),
            RhsValue::Bytes(bytes) => LhsValue::Bytes(Cow::Borrowed(bytes)),
            RhsValue::Int(integer) => LhsValue::Int(*integer),
            RhsValue::Bool(b) => match *b {},
            RhsValue::Map(m) => match *m {},
        }
    }
}

impl<'a> LhsValue<'a> {
    /// Converts a reference to an LhsValue to an LhsValue with an internal
    /// references
    pub fn as_ref(&'a self) -> Self {
        match self {
            LhsValue::Ip(ip) => LhsValue::Ip(*ip),
            LhsValue::Bytes(bytes) => LhsValue::Bytes(Cow::Borrowed(bytes)),
            LhsValue::Int(integer) => LhsValue::Int(*integer),
            LhsValue::Bool(b) => LhsValue::Bool(*b),
            LhsValue::Map(m) => LhsValue::Map(m.clone()),
        }
    }

    /// Retrieve an element from an LhsValue given a path item and a specified
    /// type.
    /// Returns a TypeMismatchError error if current type does not support it
    /// nested element. Only LhsValue::Map supports nested elements for now.
    pub fn get(
        &self,
        item: &FieldPathItem,
        ty: &Type,
    ) -> Result<Option<&LhsValue<'a>>, TypeMismatchError> {
        match (self, item) {
            (LhsValue::Map(map), FieldPathItem::Name(ref name)) => Ok(map.get(name)),
            (_, FieldPathItem::Name(_name)) => Err(TypeMismatchError {
                expected: Type::Map(Box::new(ty.clone())),
                actual: self.get_type(),
            }),
        }
    }

    /// Retrieve a mutable element from an LhsValue given a path item and a
    /// specified type.
    /// Returns a TypeMismatchError error if current type does not support
    /// nested element. Only LhsValue::Map supports nested elements for now.
    pub fn get_mut(
        &mut self,
        item: &FieldPathItem,
        ty: &Type,
    ) -> Result<Option<&mut LhsValue<'a>>, TypeMismatchError> {
        match item {
            FieldPathItem::Name(name) => match self {
                LhsValue::Map(ref mut map) => Ok(map.get_mut(name)),
                _ => Err(TypeMismatchError {
                    expected: Type::Map(Box::new(ty.clone())),
                    actual: self.get_type(),
                }),
            },
        }
    }

    /// Set an element in an LhsValue given a path item and a specified value.
    /// Returns a TypeMismatchError error if current type does not support
    /// nested element or if value type is invalid.
    /// Only LhsValyue::Map supports nested elements for now.
    pub fn set(
        &mut self,
        item: FieldPathItem,
        value: LhsValue<'a>,
    ) -> Result<Option<LhsValue<'a>>, TypeMismatchError> {
        let value_type = value.get_type();
        match item {
            FieldPathItem::Name(name) => match self {
                LhsValue::Map(ref mut map) => map.insert(name, value),
                _ => Err(TypeMismatchError {
                    expected: Type::Map(Box::new(value_type)),
                    actual: self.get_type(),
                }),
            },
        }
    }
}

declare_types!(
    /// An IPv4 or IPv6 field.
    ///
    /// These are represented as a single type to allow interop comparisons.
    Ip(IpAddr | IpAddr | IpRange),

    /// A raw bytes or a string field.
    ///
    /// These are completely interchangeable in runtime and differ only in
    /// syntax representation, so we represent them as a single type.
    Bytes(#[serde(borrow)] Cow<'a, [u8]> | Bytes | Bytes),

    /// A 32-bit integer number.
    Int(i32 | i32 | RangeInclusive<i32>),

    /// A boolean.
    Bool(bool | UninhabitedBool | UninhabitedBool),

    /// A map
    Map[Box<Type>](Map<'a> | UninhabitedMap | UninhabitedMap),
);

#[test]
fn test_lhs_value_deserialize() {
    use std::str::FromStr;

    let ipv4: LhsValue<'_> = serde_json::from_str("\"127.0.0.1\"").unwrap();
    assert_eq!(ipv4, LhsValue::Ip(IpAddr::from_str("127.0.0.1").unwrap()));

    let ipv6: LhsValue<'_> = serde_json::from_str("\"::1\"").unwrap();
    assert_eq!(ipv6, LhsValue::Ip(IpAddr::from_str("::1").unwrap()));

    let bytes: LhsValue<'_> = serde_json::from_str("\"a JSON string with unicode ❤\"").unwrap();
    assert_eq!(
        bytes,
        LhsValue::from(&b"a JSON string with unicode \xE2\x9D\xA4"[..])
    );

    let bytes =
        serde_json::from_str::<LhsValue<'_>>("\"a JSON string with escaped-unicode \\u2764\"")
            .unwrap();
    assert_eq!(
        bytes,
        LhsValue::from(&b"a JSON string with escaped-unicode \xE2\x9D\xA4"[..])
    );

    let bytes: LhsValue<'_> = serde_json::from_str("\"1337\"").unwrap();
    assert_eq!(bytes, LhsValue::from(&b"1337"[..]));

    let integer: LhsValue<'_> = serde_json::from_str("1337").unwrap();
    assert_eq!(integer, LhsValue::Int(1337));

    let b: LhsValue<'_> = serde_json::from_str("false").unwrap();
    assert_eq!(b, LhsValue::Bool(false));
}
