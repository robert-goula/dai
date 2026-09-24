# DAI (Docs AI): local documentation for you and your agents

Usage: dai <COMMAND>

Commands:
  serve     Run the daemon in the foreground (HTTP API + MCP at /mcp)
  mcp       Run an MCP server over stdio for agents (starts the daemon if needed)
  stop      Stop the running daemon
  catalog   List available docsets (DevDocs and Dash/Zeal)
  install   Download and index docsets by id (e.g. `react`, `python~3.12`, `dash:React`)
  update    Update the given docsets, or every outdated one
  remove    Remove installed docsets
  list      List installed docsets
  search    Search installed docs
  project   Show how a project's dependencies map to installed docsets
  show      Print a page as markdown
  snippet   Work with saved code snippets
  generate  Build a markdown docset for a library without a (current) docset
  context7  Find Context7 library ids (for `dai generate context7`)
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version

## Installation

```shell
cargo install --path crates/cli
```

## Generate docs

Generate documents from a repository.

```shell
dai generate repo https://github.com/TanStack/query.git --name "tanstack query"
```

Specify the docs path from the repo

```shell
git clone --depth 1 <https://github.com/TanStack/router.git> ~/repos/tanstack-router
dai generate dir ~/repos/tanstack-router/docs/start --name "tanstack start"
```
