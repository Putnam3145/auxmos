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
		let archive = TurfGrid::GAS.borrow().clone();
		for (i,(&mut cur_gas,&mut cur_archive)) in self.gas.iter_mut().zip(self.archive.iter_mut()).enumerate() {
			let adj = self.adjacency.get(i).unwrap_or(0);
			let adj_amount = adj.count_ones();
			let coeff = 1.0 / (adj_amount as f32 + 1.0);
			let mut endGas : GasMixture = GasMixture::from_vol(CELL_VOLUME);
			if adj & 1 {
				endGas = endGas + (archive.get(i+max_x) * coeff);
			}
			if adj & 2 {
				endGas = endGas + (archive.get(i-max_x) * coeff);
			}
			if adj & 4 {
				endGas = endGas + (archive.get(i+1) * coeff);
			}
			if adj & 8 {
				endGas = endGas + (archive.get(i-1) * coeff);
			}
			if adj & 16 {
				endGas = endGas + (archive.get(i+(max_y*max_x)) * coeff);
			}
			if adj & 32 {
				endGas = endGas + (archive.get(i-(max_y*max_x)) * coeff);
			}
			cur_gas.merge(endGas + (cur_archive) * (-adj_amount as f32 * coeff));
		}
	}
}
