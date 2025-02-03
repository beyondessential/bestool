#[derive(Clone, Debug)]
pub struct Context<A = (), B = ()> {
	pub args_top: A,
	pub args_sub: B,
}

impl Context {
	pub fn new() -> Self {
		Self {
			args_top: (),
			args_sub: (),
		}
	}
}

#[allow(dead_code)] // due to command features
impl<A, B> Context<A, B> {
	pub fn with_top<C>(self, args_top: C) -> Context<C, B> {
		Context::<C, B> {
			args_top,
			args_sub: self.args_sub,
		}
	}

	pub fn with_sub<C>(self, args_sub: C) -> Context<A, C> {
		Context::<A, C> {
			args_top: self.args_top,
			args_sub,
		}
	}

	pub fn push<C>(self, new_sub: C) -> Context<B, C> {
		Context::<B, C> {
			args_top: self.args_sub,
			args_sub: new_sub,
		}
	}

	pub fn take_top(self) -> (A, Context<(), B>) {
		(
			self.args_top,
			Context::<(), B> {
				args_top: (),
				args_sub: self.args_sub,
			},
		)
	}

	pub fn erased(&self) -> Context<(), ()> {
		Context::<(), ()> {
			args_top: (),
			args_sub: (),
		}
	}
}
