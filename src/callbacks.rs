use auxtools::{init, shutdown, DMResult, Proc, Value};

type DeferredFunc = Box<dyn Fn() -> DMResult + Send + Sync>;

type CallbackChannel = (flume::Sender<DeferredFunc>, flume::Receiver<DeferredFunc>);

static mut CALLBACK_CHANNELS: Option<[CallbackChannel; 3]> = None;

pub(crate) const TURFS: usize = 0;

pub(crate) const TEMPERATURE: usize = 1;

pub(crate) const ADJACENCIES: usize = 2;

#[init(partial)]
fn _start_aux_callbacks() -> Result<(), String> {
	unsafe {
		CALLBACK_CHANNELS = Some([flume::unbounded(), flume::unbounded(), flume::unbounded()]);
	}
	Ok(())
}

#[shutdown]
fn _clean_aux_callbacks() {
	unsafe { CALLBACK_CHANNELS = None }
}

fn with_aux_callback_receiver<T>(
	f: impl Fn(&flume::Receiver<DeferredFunc>) -> T,
	item: usize,
) -> T {
	f(unsafe { &CALLBACK_CHANNELS.as_ref().unwrap()[item].1 })
}

/// Returns a clone of the sender for the given callback channel.
/// # Panics
/// If callback channels have (somehow) not been initialized yet.
#[must_use]
pub fn aux_callbacks_sender(item: usize) -> flume::Sender<DeferredFunc> {
	unsafe { CALLBACK_CHANNELS.as_ref().unwrap()[item].0.clone() }
}

/// Process all the callbacks for the given channel.
/// # Panics
/// If `auxtools_stack_trace` does not exist.
pub fn process_aux_callbacks(item: usize) {
	let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	with_aux_callback_receiver(
		|receiver| {
			for callback in receiver.try_iter() {
				if let Err(e) = callback() {
					std::mem::drop(
						stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]),
					);
				}
			}
		},
		item,
	);
}
