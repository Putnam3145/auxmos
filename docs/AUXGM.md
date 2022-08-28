Auxgm is auxtools's way of consolidating gas info and allowing gases to be added at runtime. Its name is primarily a send-up of XGM, and... it mostly means the same thing, but it's subtly different.

# Requirements

Auxtools expects a global proc `/proc/_auxtools_register_gas(datum/gas/gas)` and a global variable called `gas_data`, with a list called `datums`. When `gas_data` is initialized, it should go through each individual gas datum and initialize them one-by-one, at the *very least* by adding an initialized gas variable to the `datums` list, but preferably using `_auxtools_register_gas`. Duplicated entries will overwrite the previous entry (as of 1.1.1).

Individual gas datums require the following entries:

`id`: string
`name`: string
`specific_heat`: number

...And that's it. The rest is optional.

If you're using Auxmos's built-in tile-based atmos, auxgm also requires an `overlays` list, which is initialized alongside `datums`. This can be completely empty if you want and Auxmos will run happily. Citadel initializes it like so:

```dm
		if(gas.moles_visible)
			visibility[g] = gas.moles_visible
			overlays[g] = new /list(FACTOR_GAS_VISIBLE_MAX)
			for(var/i in 1 to FACTOR_GAS_VISIBLE_MAX)
				overlays[g][i] = new /obj/effect/overlay/gas(gas.gas_overlay, i * 255 / FACTOR_GAS_VISIBLE_MAX)
		else
			visibility[g] = 0
			overlays[g] = 0
```

Everything except the overlays lines are unnecessary. However, overlays does need to be organized as seen here, as auxgm handles gas overlays.

If you're *not* using tile-based atmos, you're done; Auxmos's update_visuals is only called from a callback in auxgm's tile processing code and thus can be ignored.
