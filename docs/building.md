# Building

```sh
git clone https://github.com/Tractables/tididi-cnf
cd tididi-cnf
cargo build --release
```

The binary is `target/release/tididi`. Cargo fetches the diagram library,
vitri and their dependencies; no submodules or separate dependency checkouts
are needed. `Cargo.lock` records the versions and source revisions used by
`cargo build --release --locked`. Vitri uses a fixed Git revision because
its browser build helpers are not included in its crates.io package.
The binary builds and runs on Linux only, as the native build of [vitri](https://github.com/Tractables/vitri)
does, and [`bindings/wasm`](../bindings/wasm/README.md) builds the library for
WebAssembly through Emscripten.

## Toolchain

- **Rust.** `rust-toolchain.toml` pins the release that development and CI
  use, and rustup installs it on the first build in the repository. The oldest
  supported release is the `rust-version` in `Cargo.toml`.
- **A C++20 compiler**, GCC 12 or newer. vitri compiles CaDiCaL and the Arjun
  stack from the sources it vendors;
  [vitri's building guide](https://github.com/Tractables/vitri/blob/main/docs/building.md)
  says how it finds the compiler and how to choose another.
- **CMake** and **`pkg-config`**, to build that stack.
- **GMP, MPFR and zlib development packages.**

```sh
sudo apt install build-essential gcc-12 g++-12 cmake pkg-config libgmp-dev libmpfr-dev zlib1g-dev   # Debian/Ubuntu
sudo dnf install gcc-c++ cmake pkgconf-pkg-config gmp-devel mpfr-devel zlib-devel                   # Fedora/RHEL
```

The binary loads GMP (`libgmp` and `libgmpxx`) as a shared library, so a
machine that runs it needs GMP installed too.

The first build compiles vitri's native stack and takes a few minutes; Cargo
caches it. `cargo build --release` is the one build: the crate has no Cargo
features. The release profile links with fat LTO, and
[mold](https://github.com/rui314/mold), where it is installed, shortens that
link:

```sh
RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo build --release
```

## Use from another Rust crate

The crate is not on crates.io yet. Add it through Git in your application's
`Cargo.toml`:

```toml
[dependencies]
tididi-cnf = { git = "https://github.com/Tractables/tididi-cnf" }
```

The native toolchain and system packages above are also needed when building
the library. Cargo resolves the same dependencies for the command-line tool,
the library and the bindings.

## Documentation builds

```sh
DOCS_RS=1 cargo doc --no-deps -p tididi-cnf --open
```

With `DOCS_RS` set, vitri's `build.rs` skips the native build, and rustdoc
type-checks the crate without linking it, so the documentation needs none of
the C++ toolchain.
