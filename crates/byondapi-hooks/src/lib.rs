use std::io::Write;

pub use byondapi_impl::{bind, bind_raw_args};

pub use inventory;

pub struct Bind {
	proc_path: &'static str,
	func_name: &'static str,
	func_arguments: Option<&'static str>,
}

inventory::collect!(Bind);

pub fn generate_bindings() {
	let mut file = std::fs::File::create("./bindings.dm").unwrap();
	file.write_all(
		r#"#define AUXMOS (world.system_type == MS_WINDOWS ? "auxmos.dll" : __detect_auxmos())

/proc/__detect_auxmos()
	var/static/known_auxmos_var
	if(!known_auxmos_var)
		if (fexists("./libauxmos.so"))
			known_auxmos_var = "./libauxmos.so"
		else if (fexists("[world.GetConfig("env", "HOME")]/.byond/bin/libauxmos.so"))
			known_auxmos_var = "[world.GetConfig("env", "HOME")]/.byond/bin/libauxmos.so"
		else
			CRASH("Could not find libauxmos.so")
	return known_auxmos_var

"#
		.as_bytes(),
	)
	.unwrap();
	for thing in inventory::iter::<Bind> {
		let path = thing.proc_path;
		let func_name = thing.func_name;
		let func_arguments = thing.func_arguments.unwrap_or("");
		file.write_all(
			format!(
				r#"{path}
	call_ext(AUXMOS, "byond:{func_name}")({func_arguments})

"#
			)
			.as_bytes(),
		)
		.unwrap()
	}
	file.write_all(r#"#undef AUXMOS"#.as_bytes()).unwrap();
}
