use std::io::Write;

pub use byondapi_impl::{bind, bind_raw_args};

pub use inventory;

pub struct Bind {
	pub proc_path: &'static str,
	pub func_name: &'static str,
	pub func_arguments: &'static str,
	pub is_variadic: bool,
}

inventory::collect!(Bind);

pub fn generate_bindings() {
	_ = std::fs::remove_file("./bindings.dm");
	let mut file = std::fs::File::create("./bindings.dm").unwrap();
	file.write_all(
		r#"/* This comment bypasses grep checks */ /var/__auxmos

/proc/__detect_auxmos()
	if (world.system_type == UNIX)
		return __auxmos = "libauxmos"
	else
		return __auxmos = "auxmos"

#define AUXMOS (__auxmos || __detect_auxmos())

"#
		.as_bytes(),
	)
	.unwrap();
	for thing in inventory::iter::<Bind> {
		let path = thing.proc_path;
		let func_name = thing.func_name;
		let func_arguments = thing.func_arguments;
		let func_arguments_srcless = func_arguments
			.to_owned()
			.replace("src, ", "")
			.replace("src", "");
		if thing.is_variadic {
			//can't directly modify args, fuck you byond
			file.write_all(
				format!(
					r#"{path}(...)
	var/list/args_copy = args.Copy()
	args_copy.Insert(1, src)
	return call_ext(AUXMOS, "byond:{func_name}")(arglist(args_copy))

"#
				)
				.as_bytes(),
			)
			.unwrap()
		} else {
			file.write_all(
				format!(
					r#"{path}({func_arguments_srcless})
	return call_ext(AUXMOS, "byond:{func_name}")({func_arguments})

"#
				)
				.as_bytes(),
			)
			.unwrap()
		}
	}
}
