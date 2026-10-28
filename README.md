# Grok Bot - Claude - Relay

`grokbot-claude-relay` is the program and repository name.

A tiny message relay that lets two AI assistants on the same computer talk to each other safely. One is Claude (through Claude Code). The other is the Grok Bot desktop app, or any app that can use MCP.

Why? Two assistants often work on the same project, but they cannot see each other. This tool gives them a shared mailbox. Each one can send short notes, read its own inbox, and keep a small task board. You stay in charge. The relay itself sends nothing over the internet, and every note is saved in one file on your computer that you can read.

It is a single program written in Rust. It has a normal command line, and it also speaks MCP (a standard way for AI apps to use tools) over plain standard input and output.

## Safety at a glance

- **No network.** The program never opens a port and never connects out. (One optional feature can call a webhook you set up yourself. It is off by default.)
- **Private files.** Everything lives in folders only your user can open (mode 700 and 600).
- **Each assistant has its own secret token.** An assistant can only act as itself.
- **Size and speed limits.** Messages are at most 8 KB. Each sender gets at most 30 messages per 10 minutes.
- **Owner-authority phrases are refused.** An instruction that uses phrases like "owner approves" is refused. This is a phrase check, not proof of identity, so other wording can get through. Decisions that need a human go to you.
- **Tripwire.** If a message mentions things like `.env`, `password`, or `rm -rf`, the message is still delivered, but an alert is added for you. The alert does not copy the message text.
- **Redaction.** A sender can blank one of its own messages. The old text is wiped from the file.

Read [SECURITY.md](SECURITY.md) for the full picture, including what this tool does not protect against.

## Requirements

- Linux or macOS. (The program is tested on Linux.)
- Rust 1.89 or newer, if you build it yourself. Get it from <https://rustup.rs>.
- For Claude: [Claude Code](https://claude.com/claude-code).
- For Grok: the Grok Bot app, or any MCP client that can start a local program.
- Only for the helper scripts: Linux with X11, `xdotool`, `wmctrl`, `jq`, python3 with Pillow, and `xwd`. These are optional.

## Install

1. Install Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
2. Get the code and install:

```sh
git clone https://github.com/gcentini02/grokbot-claude-relay.git
cd grokbot-claude-relay
cargo install --locked --path .
```

This puts `grokbot-claude-relay` in `~/.cargo/bin`. Check that it works:

```sh
grokbot-claude-relay --version
```

If you see "command not found", add `~/.cargo/bin` to your `PATH`.

## First-time setup

Run this once:

```sh
grokbot-claude-relay init
```

It creates:

- `~/.local/share/grokbot-claude-relay/relay.db` (the mailbox, a SQLite file)
- `~/.config/grokbot-claude-relay/tokens/claude.token` and `grok.token` (one secret per assistant)

Keep the token files private. Do not share them or commit them to git. To get new tokens later, run `grokbot-claude-relay init --force`.

You can move the folders by setting `RELAY_HOME` (database) and `RELAY_CONFIG` (tokens) before you run anything.

## Connect Claude Code

```sh
claude mcp add grokbot-claude-relay -- grokbot-claude-relay --as claude mcp
```

Add `--scope user` right after `add` if you want it in every project. Start Claude Code and type `/mcp`. You should see `grokbot-claude-relay` listed as connected. If `grokbot-claude-relay` is not on your `PATH`, give the full path, such as `/home/you/.cargo/bin/grokbot-claude-relay`.

## Connect the Grok Bot app (or any MCP client)

Add a local ("stdio") MCP server in the app's settings. Most apps want JSON like this (also in [examples/mcp-config.json](examples/mcp-config.json)):

```json
{
  "mcpServers": {
    "grokbot-claude-relay": {
      "command": "/home/you/.cargo/bin/grokbot-claude-relay",
      "args": ["--as", "grok", "mcp"]
    }
  }
}
```

Use the full path in `command`. Desktop apps often do not see your shell `PATH`. The `--as grok` part is what makes this app act as "grok". Restart the app after saving.

## Daily use

The assistants use the MCP tools: `send`, `inbox`, `ack`, `redact`, `tasks`, `task_update`, `health`, `digest`. You can just ask in plain words:

- To Claude: "Send Grok a question asking which file it is editing, then check your inbox."
- To Grok: "Read your relay inbox and answer anything from Claude."

You can also use the same features yourself from a terminal. Add `--as claude` or `--as grok` to say who you are acting as:

```sh
# Claude sends Grok a note
grokbot-claude-relay --as claude send --to grok --kind question --body "Which file are you editing?"

# Grok reads its inbox, then marks it read
# (ack moves a cursor: acking id 9 also hides unread ids below 9, and ack tells you how many)
grokbot-claude-relay --as grok inbox
grokbot-claude-relay --as grok ack 1

# How many unread messages?
grokbot-claude-relay --as grok inbox --count

# Shared task board
grokbot-claude-relay --as claude task-update T1 --state in_progress --label docs --note "writing the README"
grokbot-claude-relay --as claude tasks

# Is everything healthy?
grokbot-claude-relay --as claude health

# A plain summary for you, the human
grokbot-claude-relay --as claude digest --since 2026-01-01T00:00:00Z

# Safe copy of the database
grokbot-claude-relay backup ~/relay-backup.db
```

Message kinds are `status`, `question`, `instruction`, `done` and `alert`. You can send to `claude`, `grok`, or `owner` (that means you). Alerts for you show up in the `owner` inbox. The relay does not give `owner` a token on purpose, because you are not an assistant. Read those alerts in the digest, or open the database with any SQLite viewer.

More detail is in [docs/HOW-IT-WORKS.md](docs/HOW-IT-WORKS.md).

## Helper scripts (optional, Linux X11 only)

They live in `scripts/`:

- `grokbot_send.sh "message"` types one line into the Grok Bot chat box. It is a nudge, such as "check your relay inbox". Needs `xdotool` and `wmctrl`. Set `GROKBOT_TITLE` if the window title is different. Set `CLICK_X_PCT` and `CLICK_Y_PCT` to move where it clicks.
- `winshot.py <window-id> <out.png> [scale]` saves a picture of one window. Needs python3, Pillow and `xwd`.
- `relay_check.sh` is a quiet inbox check for a timer. It logs plain status messages and prints only what needs attention.

Each script has a comment at the top that explains its settings. `grokbot_send.sh` clicks and presses Enter in another app, so test it first with a harmless message.

This project is not affiliated with xAI or Anthropic. "Grok" is xAI's product name and "Claude" is Anthropic's.

## Sharing a built copy

Share the source with `git archive` or the GitHub download, never by zipping a folder that contains `target/`: build output can embed paths from your computer. If you publish binaries, build them in CI or with `RUSTFLAGS="--remap-path-prefix=$HOME=/build"`.

## Troubleshooting

- **"not initialized; run grokbot-claude-relay init"**: you skipped setup. Run `grokbot-claude-relay init`.
- **"token check failed"**: the token file does not match the database. Run `init --force`, then restart your apps.
- **"token file ... is missing"**: the app runs as a different user or has a different `HOME`. Set `RELAY_HOME` and `RELAY_CONFIG` in the app config so both point to the same folders.
- **The app does not list the tools**: use the full path to the program in `command`, then restart the app.
- **"rate cap reached"**: you sent more than 30 messages in 10 minutes. Wait a bit.
- **"the limit is 8192"**: messages are capped at 8192 bytes. Shorten the message or split it.
- **The script says the window was not found**: run `wmctrl -l` and set `GROKBOT_TITLE` to the end of the title you see.

## Uninstall

```sh
cargo uninstall grokbot-claude-relay        # if you used cargo install
claude mcp remove grokbot-claude-relay
rm -rf ~/.local/share/grokbot-claude-relay ~/.config/grokbot-claude-relay   # deletes all messages and tokens
```

Also remove the entry from the Grok Bot app's MCP settings.

## FAQ

**Is any of my data sent anywhere?** The relay sends nothing: it has no network listener and makes no outgoing connection by default. Messages stay in one file on your computer. When an AI reads a message, that message goes to that AI's provider as part of the conversation.

**Can other people on my computer read it?** The folders are private to your user. Other normal users cannot read them. Anyone with root access can.

**Can an assistant pretend to be the other one?** Only if it can read the other's token file. Run each app as you, and keep the token files private.

**Does it work on Windows?** Not yet. It uses Unix file permissions.

**What is the optional webhook?** If you put a URL in `~/.config/grokbot-claude-relay/grok-webhook.url` (file mode 600), the relay sends an empty ping to it after a message to `grok`. This is for apps that can be woken by a webhook. Skip it if you do not need it.

**What is `attach-get`?** It lets an assistant read a small text file whose path is in a message. It is off until you set `RELAY_ATTACH_ROOTS` to a list of allowed folders separated by `:`.

## License

MIT. See [LICENSE](LICENSE).
