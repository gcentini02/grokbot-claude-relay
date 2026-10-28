//! Storage, identity and every relay rule. The CLI and the MCP server both call into `Session`.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::wake;

pub const PARTICIPANTS: [&str; 3] = ["claude", "grok", "owner"];
/// Participants that hold a token and can be used with `--as`.
pub const SENDERS: [&str; 2] = ["claude", "grok"];
pub const SEND_KINDS: [&str; 5] = ["status", "question", "instruction", "done", "alert"];
pub const MAX_BODY: usize = 8192;
pub const RATE_CAP: i64 = 30;
pub const SCHEMA_VERSION: i64 = 1;
pub const ATTACH_MAX: u64 = 64 * 1024;
const RELAY_SENDER: &str = "relay";
const REDACTED: &str = "[redacted]";
const OWNER_REFUSAL: &str =
    "instructions cannot carry owner authority; owner decisions go to the owner";
const OWNER_CLAIMS: [&str; 4] = [
    "owner approves",
    "owner approved",
    "owner ruling",
    "on behalf of the owner",
];
/// Tripwire terms. Matching only raises an alert to the owner; it is not the safety boundary.
const TRIPWIRE: [&str; 9] = [
    ".env",
    "id_rsa",
    "private key",
    "api_key",
    "password",
    "credential",
    "secret",
    "sudo ",
    "rm -rf",
];
/// Attachments are off unless RELAY_ATTACH_ROOTS lists allowed folders (separated by ":").
const ATTACH_ROOTS_ENV: &str = "RELAY_ATTACH_ROOTS";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipient TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('status','question','instruction','done','alert','redact')),
    ref TEXT,
    body TEXT NOT NULL,
    idem_key TEXT,
    redacted INTEGER NOT NULL DEFAULT 0,
    UNIQUE(sender, idem_key)
);
CREATE TABLE IF NOT EXISTS cursors (
    participant TEXT PRIMARY KEY,
    last_ack_id INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS participants (
    name TEXT PRIMARY KEY,
    token_sha256 TEXT NOT NULL,
    last_seen TEXT
);
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    owner TEXT,
    state TEXT,
    label TEXT,
    note TEXT,
    updated_ts TEXT,
    updated_by TEXT
);
";

/// Where the database, tokens and webhook file live. Both come from env vars so tests can use temp dirs.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let user_home =
            PathBuf::from(std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?);
        Ok(Self {
            home: env_path("RELAY_HOME")
                .unwrap_or_else(|| user_home.join(".local/share/grokbot-claude-relay")),
            config: env_path("RELAY_CONFIG")
                .unwrap_or_else(|| user_home.join(".config/grokbot-claude-relay")),
        })
    }

    pub fn db(&self) -> PathBuf {
        self.home.join("relay.db")
    }

    pub fn token(&self, name: &str) -> PathBuf {
        self.config.join("tokens").join(format!("{name}.token"))
    }

    pub fn webhook(&self) -> PathBuf {
        self.config.join("grok-webhook.url")
    }

    pub fn wake_stamp(&self) -> PathBuf {
        self.home.join("wake-alert.stamp")
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn open_db(path: &Path, flags: OpenFlags) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("open {}", path.display()))?;
    conn.busy_timeout(Duration::from_millis(5000))?;
    // Deleted and overwritten pages are zeroed, so a redacted body does not survive in free pages.
    conn.pragma_update(None, "secure_delete", "ON")?;
    Ok(conn)
}

fn now(conn: &Connection) -> Result<String> {
    Ok(
        conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
            r.get(0)
        })?,
    )
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn new_token() -> Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|e| anyhow!("random source failed: {e}"))?;
    Ok(hex::encode(buf))
}

fn make_private_dir(path: &Path) -> Result<()> {
    // Only tighten a directory this call created or one that is still empty.
    let existed = path.exists() && fs::read_dir(path)?.next().is_some();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if !existed {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, contents: &str) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// Creates the home and config dirs, the database and one token per sender. `force` rotates tokens.
pub fn init(paths: &Paths, force: bool) -> Result<()> {
    let initialized = paths.db().exists() || SENDERS.iter().any(|s| paths.token(s).exists());
    if initialized && !force {
        bail!("already initialized; rerun with --force to rotate tokens");
    }
    make_private_dir(&paths.home)?;
    make_private_dir(&paths.config)?;
    make_private_dir(&paths.config.join("tokens"))?;

    let mut conn = open_db(
        &paths.db(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    fs::set_permissions(paths.db(), fs::Permissions::from_mode(0o600))?;
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    for name in PARTICIPANTS {
        conn.execute(
            "INSERT OR IGNORE INTO cursors (participant) VALUES (?1)",
            [name],
        )?;
    }

    // Commit the hashes first, then write the token files, so a failed commit leaves no orphan token.
    let mut tokens = Vec::new();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for name in SENDERS {
        let token = new_token()?;
        tx.execute(
            "INSERT INTO participants (name, token_sha256) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET token_sha256 = excluded.token_sha256",
            params![name, sha256_hex(&token)],
        )?;
        tokens.push((name, token));
    }
    tx.commit()?;
    for (name, token) in &tokens {
        write_private_file(&paths.token(name), token)?;
    }
    Ok(())
}

/// Stores a database copy with SQLite's online backup API. Refuses to overwrite.
pub fn backup(paths: &Paths, dest: &Path) -> Result<()> {
    if !paths.db().exists() {
        bail!("not initialized; run grokbot-claude-relay init");
    }
    if dest.exists() {
        bail!("{} already exists; choose a new path", dest.display());
    }
    let src = open_db(&paths.db(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dest)?;
    let mut dst = Connection::open(dest)?;
    Backup::new(&src, &mut dst)?.run_to_completion(100, Duration::from_millis(10), None)?;
    dst.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

fn check_text(text: &str, what: &str) -> Result<()> {
    if text.is_empty() {
        bail!("{what} is empty");
    }
    if text.len() > MAX_BODY {
        bail!(
            "{what} is {} bytes; the limit is {MAX_BODY}. Shorten it or split it",
            text.len()
        );
    }
    Ok(())
}

fn tripwire_hit(text: &str) -> bool {
    let lower = text.to_lowercase();
    TRIPWIRE.iter().any(|term| lower.contains(term))
}

fn claims_owner_authority(body: &str) -> bool {
    let normalized = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    OWNER_CLAIMS.iter().any(|claim| normalized.contains(claim))
}

fn insert_message(
    conn: &Connection,
    sender: &str,
    recipient: &str,
    kind: &str,
    ref_: Option<&str>,
    body: &str,
    idem: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO messages (ts, sender, recipient, kind, ref, body, idem_key)
         VALUES (strftime('%Y-%m-%dT%H:%M:%SZ','now'), ?1, ?2, ?3, ?4, ?5, ?6)",
        params![sender, recipient, kind, ref_, body, idem],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_alert(conn: &Connection, text: &str) -> Result<i64> {
    insert_message(conn, RELAY_SENDER, "owner", "alert", None, text, None)
}

struct Msg {
    id: i64,
    ts: String,
    sender: String,
    recipient: String,
    kind: String,
    ref_: Option<String>,
    body: String,
    redacted: bool,
}

impl Msg {
    fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: r.get(0)?,
            ts: r.get(1)?,
            sender: r.get(2)?,
            recipient: r.get(3)?,
            kind: r.get(4)?,
            ref_: r.get(5)?,
            body: r.get(6)?,
            redacted: r.get::<_, i64>(7)? == 1,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "ts": self.ts,
            "sender": self.sender,
            "recipient": self.recipient,
            "kind": self.kind,
            "ref": self.ref_,
            "body": self.body,
            "redacted": self.redacted,
        })
    }

    fn line(&self) -> String {
        let reference = self
            .ref_
            .as_deref()
            .map(|r| format!(" ref={r}"))
            .unwrap_or_default();
        format!(
            "#{} {} {} -> {} [{}]{}\n  {}",
            self.id,
            self.ts,
            self.sender,
            self.recipient,
            self.kind,
            reference,
            self.body.replace('\n', "\n  ")
        )
    }
}

/// An authenticated caller. Every operation runs as `me`; the sender stored is always `me`.
pub struct Session {
    pub paths: Paths,
    conn: Connection,
    me: String,
}

impl Session {
    /// Checks the caller's token file against the stored hash and stamps `last_seen`.
    pub fn open(paths: Paths, me: &str) -> Result<Self> {
        if me == "owner" {
            bail!("owner cannot be used with --as; owner decisions go to the owner");
        }
        if !SENDERS.contains(&me) {
            bail!("unknown participant '{me}'; expected claude or grok");
        }
        if !paths.db().exists() {
            bail!("not initialized; run grokbot-claude-relay init");
        }
        let conn = open_db(&paths.db(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let token = fs::read_to_string(paths.token(me))
            .map_err(|_| anyhow!("token file for '{me}' is missing"))?;
        let stored: Option<String> = conn
            .query_row(
                "SELECT token_sha256 FROM participants WHERE name = ?1",
                [me],
                |r| r.get(0),
            )
            .optional()?;
        if stored.as_deref() != Some(sha256_hex(token.trim()).as_str()) {
            bail!("token check failed for '{me}'");
        }
        conn.execute(
            "UPDATE participants SET last_seen = ?2 WHERE name = ?1",
            params![me, now(&conn)?],
        )?;
        Ok(Self {
            paths,
            conn,
            me: me.to_string(),
        })
    }

    fn cursor(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT last_ack_id FROM cursors WHERE participant = ?1",
            [&self.me],
            |r| r.get(0),
        )?)
    }

    pub fn send(
        &mut self,
        to: &str,
        kind: &str,
        body: &str,
        ref_: Option<&str>,
        idem: Option<&str>,
    ) -> Result<i64> {
        if to == self.me {
            bail!("cannot send to yourself");
        }
        if !PARTICIPANTS.contains(&to) {
            bail!("unknown recipient '{to}'; expected claude, grok or owner");
        }
        if kind == "redact" {
            bail!("kind redact is written only by the redact command");
        }
        if !SEND_KINDS.contains(&kind) {
            bail!("unknown kind '{kind}'");
        }
        check_text(body, "body")?;
        if idem.is_some_and(|k| k.is_empty() || k.len() > 128) {
            bail!("idem key must be 1-128 characters");
        }
        if kind == "instruction" && claims_owner_authority(body) {
            bail!("{OWNER_REFUSAL}");
        }

        let me = self.me.clone();
        let id = {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(key) = idem {
                let prior: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM messages WHERE sender = ?1 AND idem_key = ?2",
                        params![me, key],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = prior {
                    return Ok(id);
                }
            }
            let recent: i64 = tx.query_row(
                "SELECT COUNT(*) FROM messages WHERE sender = ?1
                 AND ts >= strftime('%Y-%m-%dT%H:%M:%SZ','now','-10 minutes')",
                [&me],
                |r| r.get(0),
            )?;
            if recent >= RATE_CAP {
                bail!("rate cap reached: at most {RATE_CAP} sends per sender per 10 minutes");
            }
            let id = insert_message(&tx, &me, to, kind, ref_, body, idem)?;
            if tripwire_hit(body) {
                insert_alert(
                    &tx,
                    &format!("tripwire: message {id} matched a watched term; body not copied"),
                )?;
            }
            tx.commit()?;
            id
        };
        if to == "grok" {
            self.wake_grok();
        }
        Ok(id)
    }

    /// Sends the bodyless wake-up ping. Failures never fail the send; at most one owner alert per hour.
    fn wake_grok(&self) {
        if let wake::Outcome::Failed(reason) = wake::ping(&self.paths)
            && wake::claim_alert_slot(&self.paths)
        {
            let text = format!("wake-up ping to grok failed ({reason}); messages are still stored");
            let _ = insert_alert(&self.conn, &text);
        }
    }

    pub fn inbox(&self, since: Option<i64>, limit: i64, count: bool, json: bool) -> Result<String> {
        let after = match since {
            Some(id) => id,
            None => self.cursor()?,
        };
        if count {
            let n: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND id > ?2",
                params![self.me, after],
                |r| r.get(0),
            )?;
            return Ok(n.to_string());
        }
        if !(1..=500).contains(&limit) {
            bail!("--limit must be between 1 and 500");
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, sender, recipient, kind, ref, body, redacted FROM messages
             WHERE recipient = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(params![self.me, after, limit], Msg::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if json {
            let values: Vec<Value> = rows.iter().map(Msg::to_json).collect();
            return Ok(serde_json::to_string_pretty(&values)?);
        }
        Ok(rows.iter().map(Msg::line).collect::<Vec<_>>().join("\n"))
    }

    pub fn ack(&self, id: i64) -> Result<String> {
        let recipient: Option<String> = self
            .conn
            .query_row("SELECT recipient FROM messages WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?;
        match recipient {
            None => bail!("no message {id}"),
            Some(r) if r != self.me => bail!("message {id} is not addressed to {}", self.me),
            Some(_) => {}
        }
        let hidden: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND id < ?2
             AND id > (SELECT last_ack_id FROM cursors WHERE participant = ?1)",
            params![self.me, id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "UPDATE cursors SET last_ack_id = MAX(last_ack_id, ?2) WHERE participant = ?1",
            params![self.me, id],
        )?;
        if hidden > 0 {
            return Ok(format!(
                "acked through {id}; {hidden} earlier unread messages are now hidden"
            ));
        }
        Ok(format!("acked through {id}"))
    }

    /// Blanks the body of one of this caller's own messages. The row, its metadata and its id stay.
    pub fn redact(&mut self, id: i64, reason: &str) -> Result<i64> {
        check_text(reason, "reason")?;
        let me = self.me.clone();
        let rid = {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (sender, recipient, redacted): (String, String, i64) = tx
                .query_row(
                    "SELECT sender, recipient, redacted FROM messages WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(|| anyhow!("no message {id}"))?;
            if sender != me {
                bail!("only the original sender can redact message {id}");
            }
            if redacted == 1 {
                bail!("message {id} is already redacted");
            }
            tx.execute(
                "UPDATE messages SET body = ?2, redacted = 1 WHERE id = ?1",
                params![id, REDACTED],
            )?;
            let rid = insert_message(
                &tx,
                &me,
                &recipient,
                "redact",
                Some(&id.to_string()),
                reason,
                None,
            )?;
            if tripwire_hit(reason) {
                insert_alert(
                    &tx,
                    &format!(
                        "tripwire: redact {rid} reason matched a watched term; reason not copied"
                    ),
                )?;
            }
            tx.commit()?;
            rid
        };
        // Fold the WAL back so the old body is not left in the log file.
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(rid)
    }

    pub fn tasks(&self, json: bool) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT id, owner, state, label, note, updated_ts, updated_by FROM tasks ORDER BY rowid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "owner": r.get::<_, Option<String>>(1)?,
                    "state": r.get::<_, Option<String>>(2)?,
                    "label": r.get::<_, Option<String>>(3)?,
                    "note": r.get::<_, Option<String>>(4)?,
                    "updated_ts": r.get::<_, Option<String>>(5)?,
                    "updated_by": r.get::<_, Option<String>>(6)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if json {
            return Ok(serde_json::to_string_pretty(&rows)?);
        }
        let text = rows
            .iter()
            .map(|t| {
                format!(
                    "{} [{}] owner={} label={} by {} at {}: {}",
                    t["id"].as_str().unwrap_or(""),
                    t["state"].as_str().unwrap_or("-"),
                    t["owner"].as_str().unwrap_or("-"),
                    t["label"].as_str().unwrap_or("-"),
                    t["updated_by"].as_str().unwrap_or("-"),
                    t["updated_ts"].as_str().unwrap_or("-"),
                    t["note"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>();
        Ok(text.join("\n"))
    }

    pub fn task_update(
        &mut self,
        id: &str,
        state: &str,
        owner: Option<&str>,
        label: Option<&str>,
        note: &str,
    ) -> Result<String> {
        let id_ok = !id.is_empty()
            && id.len() <= 32
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if !id_ok {
            bail!("task id must be 1-32 characters: letters, digits, - _ .");
        }
        if state.is_empty() || state.len() > 64 {
            bail!("state must be 1-64 characters");
        }
        if owner.is_some_and(|o| o.len() > 64) || label.is_some_and(|l| l.len() > 64) {
            bail!("owner and label must be at most 64 bytes");
        }
        check_text(note, "note")?;
        let me = self.me.clone();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now(&tx)?;
        tx.execute(
            "INSERT INTO tasks (id, owner, state, label, note, updated_ts, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                owner = COALESCE(excluded.owner, tasks.owner),
                state = excluded.state,
                label = COALESCE(excluded.label, tasks.label),
                note = excluded.note,
                updated_ts = excluded.updated_ts,
                updated_by = excluded.updated_by",
            params![id, owner, state, label, note, ts, me],
        )?;
        if tripwire_hit(note) {
            insert_alert(
                &tx,
                &format!("tripwire: task {id} note matched a watched term; note not copied"),
            )?;
        }
        tx.commit()?;
        Ok(format!("task {id} updated"))
    }

    pub fn health(&self) -> Result<String> {
        let mut lines = Vec::new();
        for name in PARTICIPANTS {
            let seen: Option<Option<String>> = self
                .conn
                .query_row(
                    "SELECT last_seen FROM participants WHERE name = ?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()?;
            let seen = match seen {
                Some(Some(ts)) => ts,
                Some(None) => "never".to_string(),
                None => "n/a (no token)".to_string(),
            };
            let unread: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM messages m WHERE m.recipient = ?1
                 AND m.id > COALESCE((SELECT last_ack_id FROM cursors WHERE participant = ?1), 0)",
                [name],
                |r| r.get(0),
            )?;
            lines.push(format!("{name}: last_seen={seen} unread={unread}"));
        }
        let bytes = fs::metadata(self.paths.db())?.len();
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        lines.push(format!("db_bytes={bytes}"));
        lines.push(format!("schema_version={version}"));
        Ok(lines.join("\n"))
    }

    pub fn digest(&self, since: &str) -> Result<String> {
        if !since.contains('T') {
            bail!("--since must be RFC3339, e.g. 2026-01-01T00:00:00Z");
        }
        let normalized: Option<String> =
            self.conn
                .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', ?1)", [since], |r| {
                    r.get(0)
                })?;
        let normalized = normalized
            .ok_or_else(|| anyhow!("--since must be RFC3339, e.g. 2026-01-01T00:00:00Z"))?;

        let mut out = format!("relay digest since {normalized}\ncounts by kind:\n");
        let mut counts = self.conn.prepare(
            "SELECT kind, COUNT(*) FROM messages WHERE ts >= ?1 GROUP BY kind ORDER BY kind",
        )?;
        let rows = counts
            .query_map([&normalized], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            out.push_str("  (none)\n");
        }
        for (kind, n) in rows {
            out.push_str(&format!("  {kind}: {n}\n"));
        }

        out.push_str("done, alert and question messages:\n");
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, sender, recipient, kind, ref, body, redacted FROM messages
             WHERE ts >= ?1 AND kind IN ('done','alert','question') ORDER BY id",
        )?;
        let msgs = stmt
            .query_map([&normalized], Msg::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if msgs.is_empty() {
            out.push_str("  (none)\n");
        }
        for m in msgs {
            let first: String = m
                .body
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect();
            out.push_str(&format!(
                "  #{} {} {}->{} {}: {}\n",
                m.id, m.ts, m.sender, m.recipient, m.kind, first
            ));
        }
        Ok(out.trim_end().to_string())
    }

    /// Returns the text of the file a message names, if the file is inside the allowlist.
    pub fn attach_get(&self, id: i64) -> Result<String> {
        let (sender, recipient, body): (String, String, String) = self
            .conn
            .query_row(
                "SELECT sender, recipient, body FROM messages WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| anyhow!("no message {id}"))?;
        if sender != self.me && recipient != self.me {
            bail!("message {id} is not addressed to or from {}", self.me);
        }
        let named = body.trim();
        if named.is_empty() || named.contains('\n') || !named.starts_with('/') {
            bail!("message {id} body is not a single absolute path");
        }
        let path = fs::canonicalize(named)
            .map_err(|_| anyhow!("message {id} path does not resolve to a file"))?;
        let roots = std::env::var(ATTACH_ROOTS_ENV).unwrap_or_default();
        let allowed = roots
            .split(':')
            .filter(|root| !root.is_empty())
            .filter_map(|root| fs::canonicalize(root).ok())
            .any(|root| path.starts_with(&root));
        if !allowed {
            bail!(
                "path is outside the attachment allowlist (set RELAY_ATTACH_ROOTS to allow folders)"
            );
        }
        if tripwire_hit(&path.to_string_lossy()) {
            bail!("file name matches a watched term; not attached");
        }
        let file = File::open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            bail!("not a regular file");
        }
        if meta.len() > ATTACH_MAX {
            bail!("file is larger than 64 KB");
        }
        let mut buf = Vec::new();
        file.take(ATTACH_MAX + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > ATTACH_MAX {
            bail!("file grew past 64 KB while reading");
        }
        String::from_utf8(buf).map_err(|_| anyhow!("file is not UTF-8 text"))
    }
}
