use byondapi::{prelude::*, typecheck_trait::ByondTypeCheck};

use coarsetime::{Duration, Instant};

use eyre::Result;

type DeferredFunc = Box<dyn FnOnce() -> Result<()> + Send + Sync>;

type CallbackChannel = (flume::Sender<DeferredFunc>, flume::Receiver<DeferredFunc>);

pub type CallbackSender = flume::Sender<DeferredFunc>;

pub type CallbackReceiver = flume::Receiver<DeferredFunc>;

static CALLBACK_CHANNEL: std::sync::OnceLock<CallbackChannel> = std::sync::OnceLock::new();

pub fn clean_callbacks() {
	CALLBACK_CHANNEL
		.get()
		.map(|(_, rx)| rx.drain().for_each(|thing| std::mem::drop(thing)));
}

fn with_callback_receiver<T>(f: impl Fn(&flume::Receiver<DeferredFunc>) -> T) -> T {
	f(&CALLBACK_CHANNEL.get_or_init(|| flume::bounded(1_000_000)).1)
}

/// This gives you a copy of the callback sender. Send to it with try_send or send, then later it'll be processed
/// if one of the process_callbacks functions is called for any reason.
pub fn byond_callback_sender() -> flume::Sender<DeferredFunc> {
	CALLBACK_CHANNEL
		.get_or_init(|| flume::bounded(1_000_000))
		.0
		.clone()
}

/// Goes through every single outstanding callback and calls them.
fn process_callbacks() {
	//let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			if let Err(e) = callback() {
				//_ = stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
			}
		}
	})
}

/// Goes through every single outstanding callback and calls them, until a given time limit is reached.
fn process_callbacks_for(duration: Duration) -> bool {
	//let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	let timer = Instant::now();
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			if let Err(e) = callback() {
				//_ = stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
			}
			if timer.elapsed() >= duration {
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
/// #[bind("/proc/process_atmos_callbacks")]
/// fn atmos_callback_handle(remaining: ByondValue) {
///     auxcallback::callback_processing_hook(remaining)
/// }
/// ```

pub fn callback_processing_hook(time_remaining: ByondValue) -> Result<ByondValue> {
	if time_remaining.is_num() {
		let limit = time_remaining.get_number().unwrap() as u64;
		Ok(ByondValue::from(process_callbacks_for_millis(limit)))
	} else {
		process_callbacks();
		Ok(ByondValue::null())
	}
}
