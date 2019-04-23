use crate::{
    scheme::{Field, FieldPathItem, Scheme},
    types::{GetType, LhsValue, SetValueError, TypeMismatchError},
};
use std::convert::TryFrom;

/// An execution context stores an associated [`Scheme`](struct@Scheme) and a
/// set of runtime values to execute [`Filter`](::Filter) against.
///
/// It acts as a map in terms of public API, but provides a constant-time
/// index-based access to values for a filter during execution.
pub struct ExecutionContext<'e> {
    scheme: &'e Scheme,
    values: Box<[Option<LhsValue<'e>>]>,
}

impl<'e> ExecutionContext<'e> {
    /// Creates an execution context associated with a given scheme.
    ///
    /// This scheme will be used for resolving any field names and indices.
    pub fn new<'s: 'e>(scheme: &'s Scheme) -> Self {
        ExecutionContext {
            scheme,
            values: vec![None; scheme.get_field_count()].into(),
        }
    }

    /// Returns an associated scheme.
    pub fn scheme(&self) -> &'e Scheme {
        self.scheme
    }

    pub(crate) fn get_field_value_unchecked(&'e self, field: &Field<'e>) -> LhsValue<'e> {
        // This is safe because this code is reachable only from Filter::execute
        // which already performs the scheme compatibility check, but check that
        // invariant holds in the future at least in the debug mode.
        debug_assert!(self.scheme() == field.scheme());

        // For now we panic in this, but later we are going to align behaviour
        // with wireshark: resolve all subexpressions that don't have RHS value
        // to `false`.
        let mut lhs_value = self.values[field.index()].as_ref().unwrap_or_else(|| {
            panic!(
                "Field {} was registered but not given a value",
                field.name()
            );
        });
        let mut lhs_type = self.scheme.get_field(field.name()).unwrap().get_type();
        for item in &field.path {
            lhs_type = lhs_type.next().unwrap();
            lhs_value = lhs_value.get(item, &lhs_type).unwrap().unwrap_or_else(|| {
                panic!(
                    "Field {} was registered but not given a value",
                    field.name()
                );
            });
        }
        lhs_value.as_ref()
    }

    /// Sets a runtime value for a given field name.
    pub fn set_field_value<'v: 'e, V: Into<LhsValue<'v>>>(
        &mut self,
        name: &str,
        value: V,
    ) -> Result<(), TypeMismatchError> {
        let field = self.scheme.get_field(name).unwrap();
        let value = value.into();

        let field_type = field.get_type();
        let value_type = value.get_type();

        if field_type == value_type {
            self.values[field.index()] = Some(value);
            Ok(())
        } else {
            Err(TypeMismatchError {
                expected: field_type,
                actual: value_type,
            })
        }
    }

    /// Sets a runtime value for a given field name and a path
    pub fn set_field_value_with_path<'v: 'e, V: Into<LhsValue<'v>>>(
        &mut self,
        name: &str,
        path: impl IntoIterator<Item = FieldPathItem>,
        value: V,
    ) -> Result<(), SetValueError> {
        let mut iter = path.into_iter().peekable();

        if iter.peek().is_none() {
            return self
                .set_field_value(name, value)
                .map_err(SetValueError::TypeMismatch);
        }

        let field = self.scheme.get_field(name).unwrap();
        let value = value.into();

        let mut current_type = field.get_type();
        let value_type = value.get_type();

        if self.values[field.index()].is_none() {
            self.values[field.index()] = Some(
                LhsValue::try_from(current_type.clone()).map_err(SetValueError::VoidableType)?,
            );
        }

        let mut node = self.values[field.index()].as_mut().unwrap();

        while let Some(item) = iter.next() {
            current_type = current_type.next().ok_or_else(|| {
                SetValueError::TypeMismatch(TypeMismatchError {
                    expected: current_type,
                    actual: value.get_type_from_path(&mut iter),
                })
            })?;
            if iter.peek().is_some() {
                node = node
                    .get_mut_or_try_set_default(&item, &current_type)?
                    .unwrap();
            } else if current_type == value_type {
                node.set(item, value).map_err(SetValueError::TypeMismatch)?;
                return Ok(());
            } else {
                return Err(SetValueError::TypeMismatch(TypeMismatchError {
                    expected: current_type,
                    actual: value_type,
                }));
            }
        }

        unreachable!();
    }
}

#[test]
fn test_field_value_type_mismatch() {
    use crate::types::Type;

    let scheme = Scheme! { foo: Int };

    let mut ctx = ExecutionContext::new(&scheme);

    assert_eq!(
        ctx.set_field_value("foo", LhsValue::Bool(false)),
        Err(TypeMismatchError {
            expected: Type::Int,
            actual: Type::Bool
        })
    );
}
