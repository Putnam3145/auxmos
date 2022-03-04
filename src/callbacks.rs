use auxtools::*;

type DeferredFunc = Box<dyn Fn() -> DMResult + Send + Sync>;

type CallbackChannel = (flume::Sender<DeferredFunc>, flume::Receiver<DeferredFunc>);

static mut CALLBACK_CHANNELS: Option<[CallbackChannel; 2]> = None;

pub(crate) const ADJACENCIES: usize = 0;

#[init(partial)]
fn _start_aux_callbacks() -> Result<(), String> {
	unsafe {
		CALLBACK_CHANNELS = Some([flume::unbounded(), flume::unbounded()]);
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

pub fn aux_callbacks_sender(item: usize) -> flume::Sender<DeferredFunc> {
	unsafe { CALLBACK_CHANNELS.as_ref().unwrap()[item].0.clone() }
}

pub fn process_turf_adjacencies() {
	process_aux_callbacks(ADJACENCIES);
}
pub fn process_aux_callbacks(item: usize) {
	let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
	with_aux_callback_receiver(
		|receiver| {
			for callback in receiver.try_iter() {
				if let Err(e) = callback() {
					let _ = stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
				}
			}
		},
		item,
	)
}

pub fn process_aux_callbacks_threaded(item: usize) {
	let sender = auxcallback::byond_callback_sender();
	with_aux_callback_receiver(
		|receiver| {
			for callback in receiver.try_iter() {
				if let Err(e) = callback() {
					let _ = sender.try_send(Box::new(move || {
						let stack_trace = Proc::find("/proc/auxtools_stack_trace").unwrap();
						let _ =
							stack_trace.call(&[&Value::from_string(e.message.as_str()).unwrap()]);
						Ok(Value::null())
					}));
				}
			}
		},
		item,
	)
}
