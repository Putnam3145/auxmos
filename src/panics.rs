use std::fmt;
use std::panic;
use std::thread;

use auxtools::*;
use backtrace::Backtrace;
use chrono::*;
use log::LevelFilter;

struct Shim(Backtrace);

impl fmt::Debug for Shim {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		write!(fmt, "\n{:?}", self.0)
	}
}

#[init(full)]
pub fn log_init() -> Result<(), String> {
	panic::set_hook(Box::new(|info| {
		simple_logging::log_to_file(
			format!("{}-auxmos.log", Local::now().format("%F-%H-%M-%S")),
			LevelFilter::Debug,
		)
		.unwrap();
		log::info!("Commit hash: {}", env!("VERGEN_GIT_SHA"));
		log::info!("Branch: {}", env!("VERGEN_GIT_BRANCH"));

		let backtrace = Backtrace::new();

		let thread = thread::current();
		let thread = thread.name().unwrap_or("unnamed");

		let msg = match info.payload().downcast_ref::<&'static str>() {
			Some(s) => *s,
			None => match info.payload().downcast_ref::<String>() {
				Some(s) => &**s,
				None => "Box<Any>",
			},
		};

		match info.location() {
			Some(location) => {
				error!(
					target: "panic", "thread '{}' panicked at '{}': {}:{}{:?}",
					thread,
					msg,
					location.file(),
					location.line(),
					Shim(backtrace)
				);
			}
			None => {
				error!(
					target: "panic",
					"thread '{}' panicked at '{}'{:?}",
					thread,
					msg,
					Shim(backtrace)
				)
			}
		}
	}));

	Ok(())
}
