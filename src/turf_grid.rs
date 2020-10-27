use gas;

impl TurfGrid {
	thread_local! {
		static GAS: CsVec<GasMixture>,
		static ADJACENCY: CsVec<i8>
	};
}

impl TurfGrid {
	fn get_gas_by_id(&self, id: i32) -> Option<&GasMixture> {
		&gases.get(id)
	}
	fn get_gas(&self, x: i32, y: i32, z: i32) -> Option<&GasMixture> {
		&gases.get(x+max_x*y+max_x*max_y*z)
	}
	fn get_gas_list() {
		TurfGrid::GAS.with(|g| g)
	}
	fn process_turfs() {
		/*
			This is the replacement system for LINDA. LINDA requires a lot of bookkeeping,
			which, when coefficient-wise operations are this fast, is all just unnecessary overhead.
			This is a much simpler FDM system, basically like LINDA but without its most important feature (sleeping turfs).
			It can run in parallel, but doesn't yet. We'll see if it's required for performance reasons.
		*/
		// First we copy the gas list immutably, so we can be sure this is consistent.
		let archive = TurfGrid::GAS.borrow().clone();
		for (i,(&mut cur_gas,&mut cur_archive)) in self.gas.iter_mut().zip(self.archive.iter_mut()).enumerate() {
			/*
			We're checking an individual tile now. First, we get the adjacency of this tile. This is
			saved by a turf every single time it gets its adjacent turfs updated, and of course this
			processes everything in one go, blocking the byond thread until it's done (it's called from a hook),
			so it'll be nice and consistent.
			*/
			let adj = self.adjacency.get(i).unwrap_or(0);
			let adj_amount = adj.count_ones();
			/*
			If we don't multiply by this coefficient, the system is unstable, and will
			rapidly diverge and generate infinite or negative molages.
			In short: bad juju.
			*/
			let coeff = 1.0 / (adj_amount as f32 + 1.0);
			// We build the gas from each individual adjacent turf, starting from nothing.
			let mut endGas : GasMixture = GasMixture::from_vol(CELL_VOLUME);
			// NORTH (Byond is +y-up)
			if adj & 1 {
				endGas = endGas + (archive.get(i+max_x) * coeff);
			}
			// SOUTH
			if adj & 2 {
				endGas = endGas + (archive.get(i-max_x) * coeff);
			}
			// EAST
			if adj & 4 {
				endGas = endGas + (archive.get(i+1) * coeff);
			}
			// WEST
			if adj & 8 {
				endGas = endGas + (archive.get(i-1) * coeff);
			}
			// UP (I actually don't know if byond is +Z up or down, but Z up is standard, I'm reasonably sure, so.)
			if adj & 16 {
				endGas = endGas + (archive.get(i+(max_y*max_x)) * coeff);
			}
			// DOWN
			if adj & 32 {
				endGas = endGas + (archive.get(i-(max_y*max_x)) * coeff);
			}
			/*
			This is the weird bit, of course.
			We merge our gas with the combined gases of the others... plus
			our own archive, multiplied by the coefficient multiplied
			by the amount of adjacent turfs times negative 1. This is the step
			that simulates "sharing"; the negative-moled gas mix returned by the right hand side
			of the endGas + [stuff] expression below represents "gas removal" more than an
			actual gas mix. A virtual gas mix, so to speak.

			Either way: the result is that the gas mix is set to what it would be if it
			equally shared itself with the other tiles, plus kept part of itself in.

			Come to think, it may be fully possible that this is exactly equal to just
			adding all of the gas mixes together, then multiplying it by the total amount of mixes.

			Someone should do the math on that.
			*/
			cur_gas.merge(endGas + (cur_archive) * (-adj_amount as f32 * coeff));
		}
	}
}

#[hook("/turf/proc/__update_extools_adjacent_turfs")]
fn _hook_adjacent_turfs() {
	let adjacent_list = List::from(src.get("atmos_adjacent_turfs")?);
	TurfGrid::ADJACENT.with(|&mut adj| {
		*adj = 0;
		for i in 0..adjacent_list.len() {
			*adj |= adjacent_list.get(i)?.as_number()?;
		}
	})
}


#[hook("/datum/controller/subsystem/air/proc/process_active_turfs_extools")]
fn _process_turf_hook() {
	TurfGrid::process_turfs();
}
