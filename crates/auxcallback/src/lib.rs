use auxtools::*;

use std::time::{Duration, Instant};

use std::sync::{
	atomic::{AtomicBool, Ordering::Relaxed},
	Arc,
};

enum Timer {
	Fast(Arc<AtomicBool>),
	Slow(Instant, Duration),
}

impl Timer {
	fn new(time: Duration) -> Self {
		let done = Arc::new(AtomicBool::new(false));
		let thread_done = Arc::clone(&done);
		let builder = std::thread::Builder::new().name("auxcallback-timer".to_string());
		match builder.spawn(move || {
			std::thread::sleep(time);
			thread_done.store(true, Relaxed);
		}) {
			Ok(_) => Self::Fast(done),
			Err(_) => Self::Slow(Instant::now(), time),
		}
	}
	fn check(&self) -> bool {
		match self {
			Self::Fast(done) => done.load(Relaxed),
			Self::Slow(time, duration) => time.elapsed() >= *duration,
		}
	}
}

type DeferredFunc = Box<dyn FnOnce() -> Result<(), Runtime> + Send + Sync>;

type CallbackChannel = (flume::Sender<DeferredFunc>, flume::Receiver<DeferredFunc>);

static mut CALLBACK_CHANNEL: Option<CallbackChannel> = None;

#[init(partial)]
fn _start_callbacks() -> Result<(), String> {
	unsafe {
		CALLBACK_CHANNEL = Some(flume::bounded(100_000));
	}
	Ok(())
}

#[shutdown]
fn _clean_callbacks() {
	unsafe { CALLBACK_CHANNEL = None }
}

fn with_callback_receiver<T>(f: impl Fn(&flume::Receiver<DeferredFunc>) -> T) -> T {
	f(unsafe { &CALLBACK_CHANNEL.as_ref().unwrap().1 })
}

/// This gives you a copy of the callback sender. Send to it with try_send or send, then later it'll be processed
/// if one of the process_callbacks functions is called for any reason.
pub fn byond_callback_sender() -> flume::Sender<DeferredFunc> {
	unsafe { CALLBACK_CHANNEL.as_ref().unwrap().0.clone() }
}

/// Goes through every single outstanding callback and calls them.
fn process_callbacks() {
	let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			if let Err(e) = callback() {
				let _ = stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
			}
		}
	})
}

/// Goes through every single outstanding callback and calls them, until a given time limit is reached.
fn process_callbacks_for(duration: Duration) -> bool {
	let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	let timer = Timer::new(duration);
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			if let Err(e) = callback() {
				let _ = stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
			}
			if timer.check() {
				return true;
			}
		}
		false
	})
}

/// Goes through every single outstanding callback and calls them, until a given time limit in milliseconds is reached.
pub fn process_callbacks_for_millis(millis: u64) -> bool {
	process_callbacks_for(Duration::from_millis(millis))
}

/// This function is to be called from byond, preferably once a tick.
/// Calling with no arguments will process every outstanding callback.
/// Calling with one argument will process the callbacks until a given time limit is reached.
/// Time limit is in milliseconds.
/// This has to be manually hooked in the code, e.g.
/// ```
/// #[hook("/proc/process_atmos_callbacks")]
/// fn _atmos_callback_handle() {
///     auxcallback::callback_processing_hook(args)
/// }
/// ```

pub fn callback_processing_hook(args: &mut Vec<Value>) -> DMResult {
	match args.len() {
		0 => {
			process_callbacks();
			Ok(Value::null())
		}
		_ => {
			let arg_limit = args.get(0).unwrap().as_number()? as u64;
			Ok(Value::from(process_callbacks_for_millis(arg_limit)))
		}
	}
}
