# Building

```sh
git clone --recurse-submodules https://github.com/Tractables/tididi-bonsai
cd tididi-bonsai
cargo build --release
```

The binary is `target/release/tididi`. The diagram library (`tididi/`) and
vitri (`vendor/vitri/`) are git submodules, and the build needs both; in a
clone made without them, `git submodule update --init --recursive` fetches
them. Until the next release of [goatd](https://github.com/Tractables/goatd),
vitri's tree-decomposition dependency, a patch in `Cargo.toml` builds against
goatd's main, which Cargo fetches from GitHub. The binary builds and runs on
Linux only, as the native build of [vitri](https://github.com/Tractables/vitri)
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

## Documentation builds

```sh
DOCS_RS=1 cargo doc --no-deps -p tididi-bonsai --open
```

With `DOCS_RS` set, vitri's `build.rs` skips the native build, and rustdoc
type-checks the crate without linking it, so the documentation needs none of
the C++ toolchain.
