Rust-based atmospherics for Space Station 13 using [auxtools](https://github.com/willox/auxtools).

Still quite early. Monstermos seems to have an issue where it doesn't move enough gas around; if I were to guess, it's being too strict on not operating on the same turf twice. I have written a "Putnamos" process that tries to flood in a similar way, but it's much less realistic and, in general, worse.

This code relies on some byond code on [this fork of Citadel](https://github.com/Putnam3145/Citadel-Station-13/tree/auxtools-atmos). These will be documented in time (probably on the order of days).

TODO:  
Scrubbers are duplicating gases. Scrubbers and *only* scrubbers, e.g. passive vents aren't doing it. Further testing shows that it is not scrubbers per se, but rather when there is something actively pushing and actively pulling gases at the same time into turfs which can interact with one another, even over multiple ticks. For example, the supermatter chamber shows this issue, but not a scrubber and an injector connected directly to one another with a pair of holofans on top of them. Injectors from canisters where both are in a 2x1 room will *lose* some gases.  
Occasionally massive amounts of gases are lost to the aether for reasons I have never been able to see. This was also in the supermatter chamber, so it may be related to the above.
