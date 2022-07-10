use auxtools::*;

pub use auxcleanup_impl::datum_del;

use detour::RawDetour;

use std::ffi::c_void;

static mut DEL_DATUM: *const c_void = std::ptr::null();

static mut DEL_DATUM_ORIGINAL: Option<extern "C" fn(u32) -> c_void> = None;

#[init(full)]
fn del_hooking_init() -> Result<(), String> {
    let byondcore = sigscan::Scanner::for_module(BYONDCORE).unwrap();
    if cfg!(windows) {
        let ptr = byondcore
			.find(signature!(
				"55 8b ec 8b 4d 08 3b 0d ?? ?? ?? ?? 73 55 a1 ?? ?? ?? ?? 8b 04 88 85 c0 74 49 8b 50 ?? 81 fa 00 00 00 70"
			))
			.ok_or_else(|| "Couldn't find DEL_DATUM")?;

        unsafe {
            DEL_DATUM = ptr as *const c_void;
        }
    }

    if cfg!(unix) {
        let ptr = byondcore
			.find(signature!(
				"55 89 e5 53 83 ec 44 8b 45 08 3b 05 ?? ?? ?? ?? 73 2c 8b 15 ?? ?? ?? ?? 8b 0c 82 85 c9 74 1f 8b 51 ?? 81 fa 00 00 00 70"
			))
			.ok_or_else(|| "Couldn't find DEL_DATUM")?;

        unsafe {
            DEL_DATUM = ptr as *const c_void;
        }
    }

    unsafe {
        let hook = RawDetour::new(DEL_DATUM as *const (), del_datum_hook as *const ())
            .map_err(|_| "Couldn't detour DEL_DATUM")?;

        hook.enable()
            .map_err(|_| "Couldn't enable DEL_DATUM detour")?;

        DEL_DATUM_ORIGINAL = Some(std::mem::transmute(hook.trampoline()));

        std::mem::forget(hook);
    }
    Ok(())
}

pub struct DelDatumFunc(pub fn(u32));

#[no_mangle]
extern "C" fn del_datum_hook(datum_id: u32) -> c_void {
    for hook in inventory::iter::<DelDatumFunc> {
        hook.0(datum_id);
    }
    unsafe { DEL_DATUM_ORIGINAL.unwrap()(datum_id) }
}

inventory::collect!(DelDatumFunc);
