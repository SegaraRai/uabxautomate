# uabxautomate

![GitHub license](https://img.shields.io/badge/license-MIT-blue.svg)

## Overview

uabxautomate is a robust automation tool designed to streamline and optimize tasks with minimal configuration. With a focus on simplicity and efficiency, this tool empowers users to automate repetitive processes, enhancing productivity and reducing manual workload.

## Key Features

- **Ease of Use**: Simple configuration using TOML files.
- **Flexibility**: Supports a wide range of automation scenarios.
- **Performance**: Built with Rust for high performance and reliability.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install)

### Building

Clone the repository:

```sh
git clone https://github.com/SegaraRai/uabxautomate.git
cd uabxautomate
```

Build the project:

```sh
cargo build --release
```

If you are fortunate, you might be able to obtain pre-built binaries for Windows from [Actions](https://github.com/SegaraRai/uabxautomate/actions).

## Usage

uabxautomate supports two modes of execution: `extract` and `inspect`.

### Extract Mode

The `extract` command is used for extracting data based on the provided configuration file.

Usage:

```sh
./target/release/uabxautomate extract -c <CONFIG_FILE> [OPTIONS]
```

Options:

- `-c, --config <CONFIG_FILE>`: Specify the path to the configuration file.
- `-i, --incremental`: Perform incremental extraction.
- `-d, --dry`: Dry run without making any changes.
- `-r, --chdir`: Change the working directory.

Example:

```sh
./target/release/uabxautomate extract -i -c example.toml
```

#### Configuration File

The configuration file is written in TOML format and specifies the parameters for the extraction process. Below is an example configuration file:

```toml
# example.toml
src = "../assetbundles/**/*.bytes"
dest = "../extracted"

[[targets]]

type = "texture2d"
template = "{container}#{name}"
match = "^.+/spines/([^/]+)/[^#]+#(.+)$"
dest = "$1/$2.png"

[[targets]]

type = "text"
template = "{container}#{name}"
match = "^.+/spines/([^/]+)/[^#]+#(.+)$"
dest = "$1/$2"
```

### Inspect Mode

The `inspect` command is used for inspecting files.

Usage:

```sh
./target/release/uabxautomate inspect [OPTIONS] <FILES>...
```

Options:

- `-s, --only-supported`: Inspect only supported file types.
- `-h, --help`: Print help information.

Example:

```sh
./target/release/uabxautomate inspect -s path/to/file1.bytes path/to/file2.bytes
```

## Contributing

We welcome contributions! Please fork the repository and submit pull requests.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
