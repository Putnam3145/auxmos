#[macro_use]
extern crate lazy_static;

mod gas;

use dm::*;

use gas;

#[hook("/datum/gas_mixture/proc/__gasmixture_register")]
fn register_gasmixture_hook() {
	gas::GasMixtures::register_gasmixture(src);
	Ok(Value::null())
}
#[hook("/datum/gas_mixture/proc/__gasmixture_unregister")]
fn register_gasmixture_hook() {
	gas::GasMixtures::unregister_gasmixture(src);
	Ok(Value::null())
}

#[cfg(test)]
mod tests {
	#[test]
	fn it_works() {
		assert_eq!(2 + 2, 4);
	}
}
