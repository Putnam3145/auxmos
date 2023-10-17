use byondapi::prelude::*;

#[byondapi_binds::bind("/proc/__auxmos_init")]
pub fn auxmos_init() {
	super::gas::initialize_gases();
	super::types::initialize_gas_info_structs();
	super::turfs::initialize_turfs();
	Ok(ByondValue::null())
}

#[byondapi_binds::bind("/proc/__auxmos_shutdown")]
pub fn auxmos_shutdown() {
	super::gas::shut_down_gases();
	super::types::destroy_gas_info_structs();
	super::turfs::shutdown_turfs();
	super::turfs::groups::flush_groups_channel();
	super::reaction::clean_up_reaction_values();
	auxcallback::clean_callbacks();
	#[cfg(feature = "katmos")]
	{
		super::turfs::katmos::flush_equalize_channel();
	}
	Ok(ByondValue::null())
}
