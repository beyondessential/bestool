//! Subcommand-handler context: a type-keyed extensions map.
//!
//! Every subcommand level provides its own args struct into the context, and
//! leaf handlers extract whichever ancestors they need by type. This mirrors
//! the provider/consumer pattern used by HTTP frameworks like axum, and frees
//! us from carrying the parent type around in the handler's signature.
//!
//! Example:
//!
//! ```ignore
//! pub async fn run(args: LogsArgs, ctx: Context) -> Result<()> {
//!     let tamanu: &TamanuArgs = ctx.require();
//!     let top: &Args = ctx.require();
//!     ...
//! }
//! ```
//!
//! ## Provider contract
//!
//! The `subcommands!` macro inserts each level's args struct into the context
//! before dispatching, so leaf handlers can rely on every ancestor being
//! present. Inserting the same type twice replaces the earlier value (useful
//! when a level wants to override something from above).
//!
//! `require::<T>()` panics if a type wasn't provided; `get::<T>()` returns
//! `Option`. Prefer `require` in handler bodies — a missing ancestor is a
//! bug, not a user error.

use std::{
	any::{Any, TypeId, type_name},
	collections::HashMap,
	sync::Arc,
};

/// Type-keyed bag of values, threaded through subcommand dispatch.
#[derive(Clone, Default)]
pub struct Context {
	items: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl Context {
	pub fn new() -> Self {
		Self::default()
	}

	/// Insert a value, replacing any earlier value of the same type.
	pub fn provide<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
		self.items.insert(TypeId::of::<T>(), Arc::new(value));
		self
	}

	/// Borrow a previously provided value. Returns `None` if absent.
	pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
		self.items
			.get(&TypeId::of::<T>())
			.and_then(|v| v.downcast_ref::<T>())
	}

	/// Borrow a previously provided value. Panics if absent — use when the
	/// macro contract guarantees the type is present.
	pub fn require<T: Send + Sync + 'static>(&self) -> &T {
		self.get::<T>().unwrap_or_else(|| {
			panic!(
				"Context: required value of type `{}` not provided",
				type_name::<T>()
			)
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[derive(Debug, PartialEq, Eq)]
	struct A(u32);
	#[derive(Debug, PartialEq, Eq)]
	struct B(&'static str);

	#[test]
	fn provide_and_get() {
		let mut ctx = Context::new();
		ctx.provide(A(42));
		ctx.provide(B("hi"));
		assert_eq!(ctx.get::<A>(), Some(&A(42)));
		assert_eq!(ctx.get::<B>(), Some(&B("hi")));
	}

	#[test]
	fn provide_replaces() {
		let mut ctx = Context::new();
		ctx.provide(A(1));
		ctx.provide(A(2));
		assert_eq!(ctx.require::<A>(), &A(2));
	}

	#[test]
	fn missing_returns_none() {
		let ctx = Context::new();
		assert!(ctx.get::<A>().is_none());
	}

	#[test]
	#[should_panic(expected = "required value of type")]
	fn require_panics_when_missing() {
		let ctx = Context::new();
		let _: &A = ctx.require();
	}

	#[test]
	fn clone_shares_arcs() {
		let mut ctx = Context::new();
		ctx.provide(A(7));
		let cloned = ctx.clone();
		assert_eq!(cloned.require::<A>(), &A(7));
	}
}
