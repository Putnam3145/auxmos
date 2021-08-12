use auxtools::*;

use crate::gas::*;

#[hook("/proc/equalize_all_gases_in_list")]
fn _equalize_all_hook() {
	use std::collections::BTreeSet;
	let value_list = args
		.get(0)
		.ok_or_else(|| runtime!("Wrong number of args for equalize all: 0"))?
		.as_list()
		.map_err(|_| {
			runtime!(
				"Attempt to interpret non-list value as list {} {}:{}",
				std::file!(),
				std::line!(),
				std::column!()
			)
		})?;
	let gas_list: BTreeSet<usize> = (1..=value_list.len())
		.map(|i| {
			value_list
				.get(i)
				.unwrap()
				.get_number(byond_string!("_extools_pointer_gasmixture"))
				.unwrap()
				.to_bits() as usize
		})
		.collect(); // collect because get_number is way slower than the one-time allocation
	GasArena::with_all_mixtures(move |all_mixtures| {
		let mut tot = Mixture::new();
		let mut tot_vol: f64 = 0.0;
		for &id in &gas_list {
			if let Some(src_gas_lock) = all_mixtures.get(id) {
				let src_gas = src_gas_lock.read();
				tot.merge(&src_gas);
				tot_vol += src_gas.volume as f64;
			}
		}
		if tot_vol > 0.0 {
			for &id in &gas_list {
				if let Some(dest_gas_lock) = all_mixtures.get(id) {
					let dest_gas = &mut dest_gas_lock.write();
					let vol = dest_gas.volume; // don't wanna borrow it in the below
					dest_gas.copy_from_mutable(&tot);
					dest_gas.multiply((vol as f64 / tot_vol) as f32);
				}
			}
		}
	});
	Ok(Value::null())
}
