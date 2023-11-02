Rust-based atmospherics for Space Station 13 using [byondapi](https://github.com/spacestation13/byondapi-rs).

The compiled binary on Citadel is compiled for Citadel's CPU, which therefore means that it uses [AVX2 fused-multiply-accumulate](https://en.wikipedia.org/wiki/Advanced_Vector_Extensions#Advanced_Vector_Extensions_2).

Binaries in releases are without these optimizations for compatibility. But it runs slower and you might still run into issues, in that case, please build the project yourself.

You can build auxmos like any rust project, though you're gonna need clang installed. And `LIBCLANG_PATH` environment variable set to the bin path of clang in case of windows. Auxmos only supports `i686-unknown-linux-gnu` or `i686-pc-windows-msvc` targets on the build.

Use `cargo t generate_binds` to generate the `bindings.dm` file to include in your codebase, for the byond to actually use the library, or use the one on the repository here (generated with feature `katmos`).

The `master` branch is to be considered unstable; use the releases if you want to make sure it actually works. [The latest release is here](https://github.com/Putnam3145/auxmos/releases/latest).
