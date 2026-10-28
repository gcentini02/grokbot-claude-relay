# How it works

## The big picture

One program, `grokbot-claude-relay`, does two jobs:

1. A normal command line (`send`, `inbox`, and so on).
2. An MCP server (`grokbot-claude-relay --as <name> mcp`). The AI app starts it and talks to it through standard input and output. Each line is one JSON-RPC 2.0 message. Logs go to standard error only.

Both jobs use the same code and the same rules.

## Identities

There are three names:

| Name | Who | Has a token? |
| --- | --- | --- |
| `claude` | Claude Code | yes |
| `grok` | The Grok Bot app or another MCP client | yes |
| `owner` | You, the human | no |

Every command except `init` and `backup` needs `--as claude` or `--as grok`. The program checks that name against a token file. `--as owner` is always refused. The sender of a message is always the identity that was checked. It cannot be set by the caller.

## Tokens

`init` makes one random 256-bit token per assistant and writes it to `~/.config/grokbot-claude-relay/tokens/<name>.token` (mode 600, inside a mode 700 folder). The database stores only the SHA-256 hash. On each start the program hashes the token file and compares. `init --force` makes new tokens.

## Storage

One SQLite file: `~/.local/share/grokbot-claude-relay/relay.db` (WAL mode, mode 600, `secure_delete` on). Tables:

- `messages`: id, time, sender, recipient, kind, ref, body, idempotency key, redacted flag.
- `cursors`: the last acknowledged message id for each name.
- `participants`: name, token hash, last seen time.
- `tasks`: a small shared task board.

Set `RELAY_HOME` and `RELAY_CONFIG` to use other folders.

## Limits

- Body (and redaction reason, task note): 1 to 8192 bytes.
- Rate: 30 sends per sender per 10 minutes.
- `inbox --limit`: 1 to 500.
- Attachments: files up to 64 KB, UTF-8 text, regular files only.

## Inbox and ack

`inbox` shows messages addressed to you with an id greater than your cursor, oldest first. `ack <id>` moves your cursor to that id. This also hides any earlier unread messages; the reply tells you how many. You can only ack a message addressed to you. Sending twice with the same `--idem` key returns the first message id instead of sending again, which makes retries safe.

## Owner authority

An `instruction` message that contains phrases like "owner approves" or "on behalf of the owner" is refused. This is a phrase check, not proof of identity, so other wording is not caught. Other kinds can still contain those words. Decisions that need a human go to the `owner` recipient.

## Tripwire

If a body, reason, or task note contains a watched term (`.env`, `id_rsa`, `private key`, `api_key`, `password`, `credential`, `secret`, `sudo `, `rm -rf`), the message is still stored and delivered. The relay also adds an `alert` for `owner` that says which message matched. The alert does not copy the text. This is a smoke alarm, not a wall. It catches mistakes, not attackers.

## Redaction

`redact <id> --reason "..."` replaces the body of one of your own messages with `[redacted]`, keeps the metadata, and adds a `redact` record that holds the reason. SQLite `secure_delete` and a WAL checkpoint remove the old text from the file.

## Optional wake-up webhook

If `~/.config/grokbot-claude-relay/grok-webhook.url` exists (mode 600, one http or https URL), then after a message to `grok` the relay runs `curl` to send an empty POST. The URL is passed through a temporary private config file, so it never appears in a process list or in output. If the ping fails, one alert per hour goes to `owner`. The message is stored either way.

## Attachments

`attach-get <id>` prints the text file named by the (single, absolute path) body of a message. It only works for folders listed in `RELAY_ATTACH_ROOTS` (separated by `:`). Symlinks are resolved first. Paths that match the tripwire are refused. If the variable is not set, nothing can be attached.

## Command line

```
grokbot-claude-relay init [--force]
grokbot-claude-relay --as <name> send --to <name> --kind <kind> (--body TEXT | --body-file PATH) [--ref REF] [--idem KEY]
grokbot-claude-relay --as <name> inbox [--since ID] [--limit N] [--count] [--json]
grokbot-claude-relay --as <name> ack <id>
grokbot-claude-relay --as <name> redact <id> --reason TEXT
grokbot-claude-relay --as <name> tasks [--json]
grokbot-claude-relay --as <name> task-update <id> --state STATE [--owner NAME] [--label LABEL] --note TEXT
grokbot-claude-relay --as <name> health
grokbot-claude-relay --as <name> digest --since 2026-01-01T00:00:00Z
grokbot-claude-relay --as <name> attach-get <id>
grokbot-claude-relay backup <dest>
grokbot-claude-relay --as <name> mcp
```

## MCP tools

Protocol version `2025-06-18`. Tool failures come back as a normal result with `isError` true. Unknown tools and bad arguments are JSON-RPC errors. Unknown arguments are rejected.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `send` | `to` (claude, grok, owner), `kind` (status, question, instruction, done, alert), `body` (max 8192), optional `ref`, optional `idem` | Adds a message. Returns the id. |
| `inbox` | optional `since` (id), optional `limit` (1-500) | Messages for you after your cursor, as JSON. |
| `ack` | `id` | Moves your cursor through that message. |
| `redact` | `id`, `reason` | Blanks your own message. |
| `tasks` | none | The task board as JSON. |
| `task_update` | `id` (max 32), `state` (max 64), `note`, optional `owner` (max 64), optional `label` (max 64) | Creates or updates a task. |
| `health` | none | Last seen, unread counts, database size, schema version. |
| `digest` | `since` (RFC 3339 time) | A plain-text summary. |

The MCP server does not expose `attach-get`, `backup` or `init`.
