use std::io::Write;

pub use byondapi_impl::{bind, bind_raw_args};

pub use inventory;

pub struct Bind {
	pub proc_path: &'static str,
	pub func_name: &'static str,
	pub func_arguments: Option<&'static str>,
}

inventory::collect!(Bind);

pub fn generate_bindings() {
	_ = std::fs::remove_file("./bindings.dm");
	let mut file = std::fs::File::create("./bindings.dm").unwrap();
	file.write_all(
		r#"#define AUXMOS "auxmos"

//this exists because gases may be created when the MC doesn't exist yet
GLOBAL_REAL_VAR(list/__auxtools_initialized)

#define AUXTOOLS_CHECK(LIB)\
	if (!islist(__auxtools_initialized)) {\
		__auxtools_initialized = list();\
	}\
	if (!__auxtools_initialized[LIB]) {\
		if (fexists(LIB)) {\
			__auxmos_init();\
			__auxtools_initialized[LIB] = TRUE;\
		} else {\
			CRASH("No file named [LIB] found!")\
		}\
	}\

#define AUXTOOLS_SHUTDOWN(LIB)\
	if (__auxtools_initialized[LIB] && fexists(LIB)){\
		__auxmos_shutdown();\
		__auxtools_initialized[LIB] = FALSE;\
	}\

		"#
		.as_bytes(),
	)
	.unwrap();
	for thing in inventory::iter::<Bind> {
		let path = thing.proc_path;
		let func_name = thing.func_name;
		let func_arguments = thing.func_arguments.unwrap_or("");
		let func_arguments_srcless = func_arguments
			.to_owned()
			.replace("src, ", "")
			.replace("src", "");
		file.write_all(
			format!(
				r#"{path}({func_arguments_srcless})
	call_ext(AUXMOS, "byond:{func_name}")({func_arguments})

"#
			)
			.as_bytes(),
		)
		.unwrap()
	}
}
