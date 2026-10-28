mod mcp;
mod store;
mod wake;

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

use store::{MAX_BODY, Paths, Session};

#[derive(Parser)]
#[command(
    name = "grokbot-claude-relay",
    version,
    about = "Small local message relay between Claude and the Grok Bot app"
)]
struct Cli {
    /// Participant making the call: claude or grok. Checked against its token file.
    #[arg(long = "as", global = true, value_name = "PARTICIPANT")]
    as_: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create the home and config dirs, the database and one token per sender.
    Init {
        /// Rotate tokens on an existing relay.
        #[arg(long)]
        force: bool,
    },
    /// Append a message.
    Send {
        #[arg(long)]
        to: String,
        #[arg(long)]
        kind: String,
        #[arg(long = "ref")]
        ref_: Option<String>,
        #[arg(long)]
        idem: Option<String>,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long = "body-file")]
        body_file: Option<PathBuf>,
    },
    /// Show messages addressed to you after your cursor.
    Inbox {
        #[arg(long)]
        since: Option<i64>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        count: bool,
        #[arg(long)]
        json: bool,
    },
    /// Move your read cursor through a message addressed to you.
    Ack { id: i64 },
    /// Blank the body of your own message and keep its metadata.
    Redact {
        id: i64,
        #[arg(long)]
        reason: String,
    },
    /// List the task board.
    Tasks {
        #[arg(long)]
        json: bool,
    },
    /// Create or update a task.
    TaskUpdate {
        id: String,
        #[arg(long)]
        state: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        note: String,
    },
    /// Per-participant last_seen and unread counts, db size and schema version.
    Health,
    /// Plain-text summary of messages since a time, for the owner.
    Digest {
        #[arg(long)]
        since: String,
    },
    /// Print the file a message names (allowlisted paths only).
    AttachGet { id: i64 },
    /// Write a consistent copy of the database to DEST.
    Backup { dest: PathBuf },
    /// Serve MCP (JSON-RPC 2.0 over stdio, no network).
    Mcp,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn read_body_file(path: &PathBuf) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = Vec::new();
    file.take(MAX_BODY as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > MAX_BODY {
        bail!("body file is larger than {MAX_BODY} bytes; the body must fit the limit");
    }
    String::from_utf8(buf).map_err(|_| anyhow!("body file is not UTF-8 text"))
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_env()?;
    match cli.cmd {
        Cmd::Init { force } => {
            store::init(&paths, force)?;
            println!("initialized relay home and config");
            return Ok(());
        }
        Cmd::Backup { dest } => {
            store::backup(&paths, &dest)?;
            println!("backup written");
            return Ok(());
        }
        _ => {}
    }

    let me = cli
        .as_
        .ok_or_else(|| anyhow!("--as <participant> is required"))?;
    let mut session = Session::open(paths, &me)?;
    let out = match cli.cmd {
        Cmd::Send {
            to,
            kind,
            ref_,
            idem,
            body,
            body_file,
        } => {
            let body = match (body, body_file) {
                (Some(text), None) => text,
                (None, Some(path)) => read_body_file(&path)?,
                _ => bail!("give exactly one of --body or --body-file"),
            };
            session
                .send(&to, &kind, &body, ref_.as_deref(), idem.as_deref())?
                .to_string()
        }
        Cmd::Inbox {
            since,
            limit,
            count,
            json,
        } => session.inbox(since, limit, count, json)?,
        Cmd::Ack { id } => session.ack(id)?,
        Cmd::Redact { id, reason } => session.redact(id, &reason)?.to_string(),
        Cmd::Tasks { json } => session.tasks(json)?,
        Cmd::TaskUpdate {
            id,
            state,
            owner,
            label,
            note,
        } => session.task_update(&id, &state, owner.as_deref(), label.as_deref(), &note)?,
        Cmd::Health => session.health()?,
        Cmd::Digest { since } => session.digest(&since)?,
        Cmd::AttachGet { id } => session.attach_get(id)?,
        Cmd::Mcp => return mcp::serve(&mut session),
        Cmd::Init { .. } | Cmd::Backup { .. } => unreachable!("handled above"),
    };
    if !out.is_empty() {
        println!("{out}");
    }
    Ok(())
}
