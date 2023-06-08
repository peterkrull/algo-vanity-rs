# Algorand Vanity Address Generator

This command-line interface (CLI) tool is *guaranteed* the fastest tool to get you exactly the Algorand vanity address you want.

Features include
- Multi-threading, number of threads customizable by user
- Search for one or more patterns at minimal performance penalty
- Unlimited or once-per-pattern searching
- Load list of pattern strings from json file
- Automatically saves matching addresses to `vanities.json` file

# Usage
As the name suggests, a CLI tool is executed from the command-line. On linux it may be necessary to set the binary as executable in its properties. Open a terminal (or command prompt) in the same folder as the binary and type `./algo-vanity-rs -h` on Linux or `algo-vanity-rs -h` on Windows, which will run the binary and show the help prompt. Otherwise the using the tool is as simple as writing which patterns you want to look for, such as `algo-vanity-rs algo rand rocks` which will look for addresses which begin with `ALGO`, `RAND` and `ROCKS`.

```bash
algo-vanity-rs algo rand rocks
```

Alternatively, it is possible to create a json file, containing a list of strings to use as patterns. This may be more convenient for some uses cases. For example, if a file named `vanity_list.json` exists in the same directory as the binary, and it contains `["algo","rand","rocks"]`, then executing the following command will give the same result as the previous command.
```bash
algo-vanity-rs vanity_list.json
```

By default the tool will run indefinitely until interrupted by the user (Ctrl-C), automatically detect the number of available threads and only look for patterns in the beginning of the address. All of this can be configured, and is explained further in the `-h` prompt.

### How fast?
Thanks to a random number generator (rng) hack, we can get away with generating significantly fewer random seeds. Instead of generating 32 bytes each iteration, we can generate just 32 once per 10000, iterations, and simply perturb a few of the seed indenes. This hack alone doubles the number of addresses/second on my machine, allowing me to reach 215k addresses/second on a 10+ year old i5-3570k. I think that is impressive. The Rust language is also to thank for this speed, due to its easy multi-threading workflow.

## Build from source

To build from source you will need the [Rust toolchain](https://rustup.rs/). Clone this repository, and run `cargo build --release` to compile an executable binary file. This may take a few minutes. The binary `algo-vanity-rs` will be located in `./target/release/`.

## Download pre-compiled binaries
For safety-reasons, it is recommended to build from source. However, pre-build binaries for x64 Linux and Windows platforms are provided under the `Releases` section of the repository. These releases are not guaranteed to be up to date.
