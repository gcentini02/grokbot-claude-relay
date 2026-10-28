# Security

## What this tool is for

It keeps two assistants on one computer from passing messages in an unsafe way. It is a small local tool, not a security product.

## Threat model

It tries to protect against:

- **One assistant pretending to be the other.** Each has its own random token. The database holds only hashes.
- **An assistant pretending to be you.** `owner` has no token and cannot be used with `--as`. Instructions that claim your approval are refused.
- **Message floods.** 8 KB per message and 30 sends per sender per 10 minutes.
- **Accidental leaks.** A tripwire raises an alert when a message mentions sensitive-looking words. Alerts never copy the text.
- **Leftover secrets after a mistake.** `redact` wipes the body, including free pages and the write-ahead log.
- **Network attacks.** The program has no network listener and makes no outgoing connection (except the optional webhook you configure).
- **Casual access by other users.** Folders are mode 700, files are mode 600.

## What it does NOT protect against

- **Anyone who runs as your user, or as root.** They can read the tokens and the database.
- **A malicious assistant that can read files.** If an assistant can read the other one's token file, it can act as the other. Run the assistants with care and do not give them access to `~/.config/grokbot-claude-relay`.
- **Prompt injection.** A message is just text. The receiving assistant might follow bad instructions inside it. The relay does not understand meaning. Treat messages from the relay like any other untrusted input.
- **Clever evasion of the tripwire.** It is a simple word list. It is not the safety boundary.
- **Messages in transit inside the computer.** Nothing is encrypted at rest. Use full-disk encryption if that matters to you.
- **Stolen backups.** A `backup` copy contains all messages.
- **Windows.** Not supported. File permissions work differently there.

## Good habits

- Never put real passwords or API keys in messages.
- Never commit token files, `*.db` files, or the webhook URL file to git. The included `.gitignore` helps.
- Rotate tokens with `init --force` if you think they leaked.
- Leave `RELAY_ATTACH_ROOTS` unset unless you need it, and keep it to one small folder.

## Reporting a problem

Please use GitHub's private vulnerability reporting: on the repository page, open the **Security** tab and choose **Report a vulnerability**. If that is not available, open an issue that says you have a security report, but do not put details in it. Please give us a reasonable time to fix the problem before you share it widely.
