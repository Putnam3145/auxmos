Rust-based atmospherics for Space Station 13 using [auxtools](https://github.com/willox/auxtools).

Still super early. Monstermos doesn't seem to be working properly and I cannot figure out where the error in the logic is.

There's some manner of bug that causes the arrivals shuttle to get extremely cold immediately. My guess is that ChangeTurf is interacting badly.

This code relies on some byond code on [this fork of Citadel](https://github.com/Putnam3145/Citadel-Station-13/tree/auxtools-atmos). These will be documented in time (probably on the order of days).

TODO:
1. Holofans just don't work. This is a problem. Almost definitely to do with not updating *adjacent* tiles as well as current.
2. Sometimes space gets unimmutable'd. Big why.
3. Scrubbers cause NaN stuff weirdly often.
4. Pirate cutter temperature just keeps going up. Figure that one out.
5. Related to 4, might be its cause: windows get really, really hot.
6. Supermatter is funny.
