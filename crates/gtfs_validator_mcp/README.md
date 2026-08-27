# GTFS Guru MCP

[![Crates.io](https://img.shields.io/crates/v/gtfs-guru-mcp.svg)](https://crates.io/crates/gtfs-guru-mcp)

A read-only, validation-focused [MCP](https://modelcontextprotocol.io) server
for [GTFS Guru](https://github.com/abasis-ltd/gtfs.guru), letting an LLM
client validate GTFS feeds and inspect the resulting notices, profile facts,
and explanations directly.

## Installation

```bash
cargo install gtfs-guru-mcp
```

## Usage

```bash
gtfs-guru-mcp --transport stdio --allow-dir /path/to/gtfs/feeds
```

Point your MCP client (Claude Desktop, Claude Code, etc.) at the resulting
stdio server, or run `--transport http` with `--bind` for HTTP access
(requires a bearer token). Run `gtfs-guru-mcp --help` for the full option
list.

## Tools

| Tool | Purpose |
| --- | --- |
| `list_gtfs_feeds` | Discover feeds under the `--allow-dir` roots, with the absolute path the other tools take |
| `validate_gtfs` | Validate a feed and return exact grouped notice totals plus concrete examples |
| `explain_gtfs` | A plain-language summary derived only from deterministic profile facts |
| `get_notice_details` | Full description and severity of a single notice code |

File access is confined to the `--allow-dir` roots. Public URL downloads are
off unless you pass `--allow-url`.

The [LLM Guide](https://abasis-ltd.github.io/gtfs.guru/llm/#mcp-server) has
client configuration examples, the HTTP transport's limits, and the response
shape.

## License

Apache-2.0
