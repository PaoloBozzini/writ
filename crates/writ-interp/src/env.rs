//! The evaluation environment: lexical scopes of variable bindings.
//!
//! There is no global mutable state — an [`Env`] is a local value threaded by
//! `&mut` through evaluation, so a change in one call frame cannot reach into a
//! distant one. That is Writ's locality principle enforced at the interpreter
//! level.

use std::collections::HashMap;

use crate::value::Value;

/// A single variable binding: its value and whether it may be rebound.
#[derive(Debug, Clone)]
struct Binding {
    value: Value,
    mutable: bool,
}

/// A stack of lexical scopes. Name lookup searches from the innermost scope
/// outward, so an inner scope may shadow an outer one.
#[derive(Debug, Default)]
pub struct Env {
    scopes: Vec<HashMap<String, Binding>>,
}

impl Env {
    /// A fresh environment with a single (global) scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Enter a new innermost scope.
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Leave the innermost scope, discarding its bindings.
    pub fn pop_scope(&mut self) {
        // Never pop the global scope; that would be a bug in the caller.
        debug_assert!(self.scopes.len() > 1, "popped the global scope");
        self.scopes.pop();
    }

    /// Resolve a name through lexical scope, innermost first.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(b) = scope.get(name) {
                return Some(b.value.clone());
            }
        }
        None
    }

    /// Introduce a binding in the current (innermost) scope.
    ///
    /// Binding a name already present in the current scope is refused unless the
    /// existing binding was declared mutable — that is the "immutable by
    /// default, rebinding is a checked error" rule. A binding of the same name
    /// in an *outer* scope is not a conflict: the new binding shadows it.
    ///
    /// # Errors
    /// Returns a message if the name is already bound immutably in this scope.
    pub fn define(&mut self, name: &str, value: Value, mutable: bool) -> Result<(), String> {
        let scope = self.scopes.last_mut().expect("at least one scope");
        if let Some(existing) = scope.get(name) {
            if !existing.mutable {
                return Err(format!(
                    "cannot rebind `{name}`: it is immutable (declare it with `let mut` to allow rebinding)"
                ));
            }
            // Rebinding a mutable binding updates its value; it stays mutable.
            scope.insert(
                name.to_string(),
                Binding {
                    value,
                    mutable: true,
                },
            );
        } else {
            scope.insert(name.to_string(), Binding { value, mutable });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_through_lexical_scope() {
        let mut env = Env::new();
        env.define("x", Value::Int(1), false).unwrap();
        env.push_scope();
        // Outer binding is visible in the inner scope.
        assert_eq!(env.lookup("x"), Some(Value::Int(1)));
        // A new inner binding shadows it without conflict.
        env.define("x", Value::Int(2), false).unwrap();
        assert_eq!(env.lookup("x"), Some(Value::Int(2)));
        env.pop_scope();
        // The shadow does not leak out.
        assert_eq!(env.lookup("x"), Some(Value::Int(1)));
    }

    #[test]
    fn immutable_rebind_in_same_scope_is_refused() {
        let mut env = Env::new();
        env.define("x", Value::Int(1), false).unwrap();
        let err = env.define("x", Value::Int(2), false).unwrap_err();
        assert!(err.contains("immutable"), "{err}");
    }

    #[test]
    fn mutable_rebind_is_allowed() {
        let mut env = Env::new();
        env.define("x", Value::Int(1), true).unwrap();
        env.define("x", Value::Int(2), false).unwrap();
        assert_eq!(env.lookup("x"), Some(Value::Int(2)));
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        let env = Env::new();
        assert_eq!(env.lookup("nope"), None);
    }
}
