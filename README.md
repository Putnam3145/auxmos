Rust-based atmospherics for Space Station 13 using [auxtools](https://github.com/willox/auxtools).

Still quite early. Monstermos seems to have an issue where it doesn't move enough gas around; if I were to guess, it's being too strict on not operating on the same turf twice. I have written a "Putnamos" process that tries to flood in a similar way, but it's much less realistic and, in general, worse.

This code relies on some byond code on [this fork of Citadel](https://github.com/Putnam3145/Citadel-Station-13/tree/auxtools-atmos). These will be documented in time (probably on the order of days).

FIXED:
Duping gases was actually a race condition of sorts. It was supposed to do all the gas checks before even starting with setting the gases, but it wasn't. Fixed. Massive amounts of gases being lost was probably this or putnamos just being bad.

TODO:
I broke reactions. Most likely fix is just sorting them by priority instead of by string.
