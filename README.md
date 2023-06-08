# Algorand Vanity Address Generator

This command-line interface (CLI) tool is the fastest tool to get you exactly the vanity address you want. 

Features include
- Multi-threading, number of threads customizable by user
- Search for one or more patterns at minimal performance penalty
- Unlimited or once-per-pattern searching
- Automatically saves matching addresses to `vanities.json` file

# Usage
As the name suggests, a CLI tool is executed from the command-line. On linux it may be necessary to set the binary as executable in its properties. Open a terminal (or command prompt) in the same folder as the binary and type `./algo-vanity-rs -h` on Linux or `algo-vanity-rs -h` on Windows, which will run the binary and show the help prompt. Otherwise the using the tool is as simple as writing which patterns you want to look for, such as `algo-vanity-rs algo rand rocks` which will look for addresses which begin with `ALGO`, `RAND` and `ROCKS`.

By default the tool will run indefinitely until interrupted by the user, automatically detect the number of available threads and only look for patterns in the beginning of the address. All of this can be configured, and is explained in the `-h` prompt.

## Build from source

To build from source you will need the [Rust toolchain](https://rustup.rs/). Clone this repository, and run `cargo build --release` to compile an executable binary file. This may take a few minutes. The binary `algo-vanity-rs` will be located in `./target/release/`.

## Download pre-compiled binaries
For safety-reasons, it is recommended to build from source. However, pre-build binaries for x64 Linux and Windows platforms are provided under the `Releases` section of the repository. These releases are not guaranteed to be up to date.

