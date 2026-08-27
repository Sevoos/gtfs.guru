# Installation

Pick the front-end that matches how you work:

| You want to… | Install | Section |
| --- | --- | --- |
| Drag a `gtfs.zip` onto a window | Desktop app | [Desktop app](#desktop-app) |
| Validate from a terminal or CI job | CLI binary | [Command-line tool](#command-line-tool) |
| Check feeds inside a notebook or ETL job | `pip install gtfs-guru` | [Python package](#python-package) |
| Let an LLM client validate feeds | MCP server | [MCP server](#mcp-server) |
| Build from a checkout | `cargo build` | [From source](#from-source-rust) |

!!! info "Versioning"
    The engine, CLI, report, model, profile, MCP, Python, WASM, and web crates
    are versioned and released together, so a `gtfs-guru` you install from any
    channel reports the same version. Desktop app builds are tagged on GitHub.
    The current number is on
    [crates.io](https://crates.io/crates/gtfs-guru) and in
    `gtfs-guru --version`.

## Desktop app

The easiest way to validate a feed without touching a command line. Download the
installer for your OS from the
[latest release](https://github.com/abasis-ltd/gtfs.guru/releases/latest) — these
links always resolve to the newest build:

| Platform | Download |
| --- | --- |
| 🍎 macOS | [`gtfs-guru-macos.dmg`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-macos.dmg) |
| 🪟 Windows (x64) | [`gtfs-guru-windows-x64.msi`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-windows-x64.msi) · [`…-setup.exe`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-windows-x64-setup.exe) |
| 🐧 Linux (Debian/Ubuntu) | [`gtfs-guru-linux-amd64.deb`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-linux-amd64.deb) |
| 🐧 Linux (portable) | [`gtfs-guru-linux-amd64.AppImage`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-linux-amd64.AppImage) |

Run the installer, launch the app, and drop your `gtfs.zip` onto the window.

## Command-line tool

### Installer script

```bash
curl -fsSL https://raw.githubusercontent.com/abasis-ltd/gtfs.guru/main/scripts/install.sh | bash
```

```powershell
iwr -useb https://raw.githubusercontent.com/abasis-ltd/gtfs.guru/main/scripts/install.ps1 | iex
```

Both scripts honour a few environment variables:

| Variable | Effect |
| --- | --- |
| `INSTALL_DIR` | Install somewhere other than `~/.local/bin` |
| `GTFS_GURU_LINUX_FLAVOR` | `gnu` or `musl` (x86_64 Linux only) |
| `GTFS_GURU_VERSION` | Pin a release, e.g. `v1.0.0` |

### Prebuilt archives

| Platform | Download |
| --- | --- |
| macOS (arm64) | [`gtfs-guru-macos-arm64.tar.gz`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-macos-arm64.tar.gz) |
| macOS (x86_64) | [`gtfs-guru-macos-x86_64.tar.gz`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-macos-x86_64.tar.gz) |
| Linux (x86_64, glibc) | [`gtfs-guru-linux-x86_64.tar.gz`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-linux-x86_64.tar.gz) |
| Linux (x86_64, musl) | [`gtfs-guru-linux-x86_64-musl.tar.gz`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-linux-x86_64-musl.tar.gz) |
| Linux (arm64) | [`gtfs-guru-linux-aarch64.tar.gz`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-linux-aarch64.tar.gz) |
| Windows (x64) | [`gtfs-guru-windows-x64.zip`](https://github.com/abasis-ltd/gtfs.guru/releases/latest/download/gtfs-guru-windows-x64.zip) |

### From crates.io

```bash
cargo install gtfs-guru
```

## Python package

Requires Python 3.8 or newer.

```bash
pip install gtfs-guru
```

```python
import gtfs_guru

report = gtfs_guru.validate("path/to/gtfs.zip")
print(f"Valid: {report.is_valid}, notices: {len(report.notices)}")

report.save_html("validation_report.html")
report.save_json("report.json")
```

See the [Python API](python_api.md) reference for the full surface.

To build the wheel from a checkout:

```bash
cd crates/gtfs_validator_python
pip install maturin
maturin build --release
pip install target/wheels/gtfs_guru-*.whl
```

## MCP server

```bash
cargo install gtfs-guru-mcp
```

Prebuilt `gtfs-guru-mcp` archives are attached to every
[release](https://github.com/abasis-ltd/gtfs.guru/releases/latest) for Linux
(x86_64, x86_64-musl, aarch64), macOS (x86_64, arm64, universal), and Windows
(x64). The [LLM Guide](llm.md#mcp-server) covers client configuration, the
exposed tools, and the HTTP transport.

## From source (Rust)

On Linux, install the [system dependencies](system-dependencies.md) first.

```bash
git clone https://github.com/abasis-ltd/gtfs.guru
cd gtfs.guru
cargo build --release
```

Binaries land in `target/release/`:

- `gtfs-guru` — command-line tool
- `gtfs-guru-web` — web API server
- `gtfs-guru-mcp` — MCP server

To build only the CLI: `cargo build --release -p gtfs-guru`.
