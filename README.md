Rust-based atmospherics for Space Station 13 using [auxtools](https://github.com/willox/auxtools).

Still quite early. Monstermos has an annoying anisotropy--it prefers to go left and right rather than up or down. Up and down are first in the adjacency bitfield (little endian wise), so this isn't *terribly* surprising, but it is annoying. Perhaps it's a problem with the algorithm--am I using a stack instead of a queue?

This code relies on some byond code on [this fork of Citadel](https://github.com/Putnam3145/Citadel-Station-13/tree/auxtools-atmos). Documentation on this is associated with the individual data structures that hold them, in this repository.

The compiled binary on Citadel is compiled for Citadel's CPU, which therefore means that it uses [AVX2 fused-multiply-accumulate](https://en.wikipedia.org/wiki/Advanced_Vector_Extensions#Advanced_Vector_Extensions_2). Yes, really. If you have issues, compile it yourself, via `cargo rustc --target=i686-pc-windows-msvc --release --features "all_reaction_hooks" -- -C target-cpu=native`.

TODO:
I would quite a lot like monstermos to work.
