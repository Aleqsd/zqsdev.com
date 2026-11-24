//! Every SQL statement of the ledger, shared by the Durable Object store and the native test
//! store. Placeholders are plain `?`, bound in textual order. No statement scans `questions` or
//! `commands` without a bound (checked by `EXPLAIN QUERY PLAN` tests).

/// Schema version 1, created with `IF NOT EXISTS` (additive changes only: rollbacks must work).
pub const SCHEMA: [&str; 6] = [
    "CREATE TABLE IF NOT EXISTS meta (
  id                 INTEGER PRIMARY KEY CHECK (id = 1),
  schema_version     INTEGER NOT NULL,
  min_reader_version INTEGER NOT NULL,
  next_seq           INTEGER NOT NULL,
  edge_minute INTEGER NOT NULL, sum_minute INTEGER NOT NULL,
  edge_hour   INTEGER NOT NULL, sum_hour   INTEGER NOT NULL,
  edge_day    INTEGER NOT NULL, sum_day    INTEGER NOT NULL,
  edge_month  INTEGER NOT NULL, sum_month  INTEGER NOT NULL,
  inflight           TEXT    NOT NULL,
  calls_day          INTEGER NOT NULL,
  calls_today        INTEGER NOT NULL,
  ip_count           INTEGER NOT NULL,
  ips_pruned_at      INTEGER NOT NULL,
  cmd_next_seq       INTEGER NOT NULL,
  cmd_day            INTEGER NOT NULL,
  cmd_today          INTEGER NOT NULL,
  cmd_last_at        INTEGER NOT NULL,
  text_bytes         INTEGER NOT NULL,
  text_dropped       INTEGER NOT NULL,
  maintained_at      INTEGER NOT NULL,
  import_closed      INTEGER NOT NULL
)",
    "CREATE TABLE IF NOT EXISTS questions (
  seq          INTEGER PRIMARY KEY,
  qid          TEXT    NOT NULL,
  at           INTEGER NOT NULL,
  nusd         INTEGER NOT NULL,
  state        INTEGER NOT NULL,
  lease_until  INTEGER NOT NULL,
  ip           TEXT,
  question     TEXT,
  question_len INTEGER,
  answer_id    TEXT,
  answer       TEXT,
  answer_len   INTEGER,
  model        TEXT,
  finished_at  INTEGER,
  usage        TEXT,
  latency_ms   INTEGER,
  error        TEXT
)",
    "CREATE INDEX IF NOT EXISTS questions_at_nusd ON questions(at, nusd)",
    "CREATE TABLE IF NOT EXISTS ip_windows (
  ip_key  TEXT PRIMARY KEY,
  hits    TEXT    NOT NULL,
  last_at INTEGER NOT NULL
) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS commands (
  seq         INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  command     TEXT,
  command_len INTEGER NOT NULL,
  mode        TEXT    NOT NULL,
  ip          TEXT    NOT NULL
)",
    "CREATE TABLE IF NOT EXISTS imported (
  id  TEXT PRIMARY KEY,
  seq INTEGER NOT NULL
) WITHOUT ROWID",
];

// ---------------------------------------------------------------- meta

pub const META_SELECT: &str = "SELECT schema_version, min_reader_version, next_seq, \
edge_minute, sum_minute, edge_hour, sum_hour, edge_day, sum_day, edge_month, sum_month, \
inflight, calls_day, calls_today, ip_count, ips_pruned_at, cmd_next_seq, cmd_day, cmd_today, \
cmd_last_at, text_bytes, text_dropped, maintained_at, import_closed FROM meta WHERE id = 1";

/// 24 parameters in the column order of `META_SELECT`; one row written.
pub const META_UPSERT: &str = "INSERT INTO meta (id, schema_version, min_reader_version, \
next_seq, edge_minute, sum_minute, edge_hour, sum_hour, edge_day, sum_day, edge_month, \
sum_month, inflight, calls_day, calls_today, ip_count, ips_pruned_at, cmd_next_seq, cmd_day, \
cmd_today, cmd_last_at, text_bytes, text_dropped, maintained_at, import_closed) \
VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
ON CONFLICT(id) DO UPDATE SET schema_version = excluded.schema_version, \
min_reader_version = excluded.min_reader_version, next_seq = excluded.next_seq, \
edge_minute = excluded.edge_minute, sum_minute = excluded.sum_minute, \
edge_hour = excluded.edge_hour, sum_hour = excluded.sum_hour, \
edge_day = excluded.edge_day, sum_day = excluded.sum_day, \
edge_month = excluded.edge_month, sum_month = excluded.sum_month, \
inflight = excluded.inflight, calls_day = excluded.calls_day, \
calls_today = excluded.calls_today, ip_count = excluded.ip_count, \
ips_pruned_at = excluded.ips_pruned_at, cmd_next_seq = excluded.cmd_next_seq, \
cmd_day = excluded.cmd_day, cmd_today = excluded.cmd_today, cmd_last_at = excluded.cmd_last_at, \
text_bytes = excluded.text_bytes, text_dropped = excluded.text_dropped, \
maintained_at = excluded.maintained_at, import_closed = excluded.import_closed";

// ---------------------------------------------------------------- questions (ledger + AI log)

pub const QUESTIONS_ANY: &str = "SELECT 1 FROM questions LIMIT 1";
pub const QUESTIONS_MAX_SEQ: &str = "SELECT MAX(seq) FROM questions";
pub const QUESTIONS_MIN_SEQ: &str = "SELECT MIN(seq) FROM questions";

/// Window expiry: `?` edge, `?` new edge (covering index range).
pub const SUM_RANGE: &str = "SELECT COALESCE(SUM(nusd), 0) FROM questions WHERE at > ? AND at <= ?";
/// Rebuild: `?` edge.
pub const SUM_AFTER: &str = "SELECT COALESCE(SUM(nusd), 0) FROM questions WHERE at > ?";

pub const QUESTION_HEAD: &str = "SELECT at, nusd, state FROM questions WHERE seq = ?";

pub const QUESTION_INSERT: &str = "INSERT INTO questions (seq, qid, at, nusd, state, \
lease_until, ip, question, question_len) VALUES (?, ?, ?, ?, 0, ?, ?, ?, ?)";

/// state, nusd, finished_at, answer_id, answer, answer_len, model, usage, latency_ms, error, seq.
pub const QUESTION_FINISH: &str = "UPDATE questions SET state = ?, nusd = ?, lease_until = 0, \
finished_at = ?, answer_id = ?, answer = ?, answer_len = ?, model = ?, usage = ?, \
latency_ms = ?, error = ? WHERE seq = ?";

pub const QUESTION_SET_NUSD: &str = "UPDATE questions SET nusd = ? WHERE seq = ?";

/// Imported VPS charge: seq, qid, at, nusd.
pub const IMPORTED_QUESTION_INSERT: &str = "INSERT INTO questions (seq, qid, at, nusd, state, \
lease_until) VALUES (?, ?, ?, ?, 9, 0)";

/// Rebuild: in-flight leases. `?` now − 60, `?` now.
pub const INFLIGHT_ROWS: &str = "SELECT seq, lease_until FROM questions \
WHERE at > ? AND state = 0 AND lease_until > ?";

/// Rebuild: admitted calls since the start of the UTC day.
pub const CALLS_SINCE: &str = "SELECT COUNT(*) FROM questions WHERE at >= ? AND state != 9";

/// Stats: questions of the last 24 h without a known IP.
pub const UNKNOWN_IP_QUESTIONS: &str =
    "SELECT COUNT(*) FROM questions WHERE at > ? AND ip = 'unknown'";

/// Retention and purge: `?` cutoff, `?` batch size. Returns the text bytes of each deleted row.
pub const QUESTIONS_DELETE_UPTO: &str = "DELETE FROM questions WHERE seq IN \
(SELECT seq FROM questions WHERE at <= ? ORDER BY at, seq LIMIT ?) \
RETURNING COALESCE(length(CAST(question AS BLOB)), 0) + COALESCE(length(CAST(answer AS BLOB)), 0)";

/// Export page: `?` after seq, `?` limit.
pub const EXPORT_QUESTIONS: &str = "SELECT seq, qid, at, nusd, state, lease_until, ip, question, \
question_len, answer_id, answer, answer_len, model, finished_at, usage, latency_ms, error \
FROM questions WHERE seq > ? ORDER BY seq LIMIT ?";

/// Ledger export page in `(at, seq)` order: `?` cursor at, `?` month edge, `?` cursor at,
/// `?` cursor seq, `?` limit.
pub const EXPORT_LEDGER: &str = "SELECT seq, qid, at, nusd FROM questions \
WHERE at >= ? AND at > ? AND (at > ? OR seq > ?) ORDER BY at, seq LIMIT ?";

// ---------------------------------------------------------------- per-IP windows

pub const IP_HITS: &str = "SELECT hits FROM ip_windows WHERE ip_key = ?";
pub const IP_UPSERT: &str = "INSERT INTO ip_windows (ip_key, hits, last_at) VALUES (?, ?, ?) \
ON CONFLICT(ip_key) DO UPDATE SET hits = excluded.hits, last_at = excluded.last_at";
/// `?` now − 86400 (≤ 4,096 rows scanned).
pub const IP_PRUNE: &str = "DELETE FROM ip_windows WHERE last_at <= ? RETURNING 1";
pub const IP_COUNT: &str = "SELECT COUNT(*) FROM ip_windows";

// ---------------------------------------------------------------- commands

pub const COMMAND_INSERT: &str = "INSERT INTO commands (seq, at, command, command_len, mode, ip) \
VALUES (?, ?, ?, ?, ?, ?)";
pub const COMMANDS_MAX_SEQ: &str = "SELECT MAX(seq) FROM commands";
pub const COMMANDS_MIN_SEQ: &str = "SELECT MIN(seq) FROM commands";
/// Window reload: the newest `?` timestamps.
pub const COMMAND_RECENT_AT: &str = "SELECT at FROM commands ORDER BY seq DESC LIMIT ?";
/// Rebuild: commands since the start of the UTC day among the last 20,000 seqs.
pub const COMMANDS_SINCE: &str = "SELECT COUNT(*) FROM commands WHERE seq > ? AND at >= ?";
/// Stats: commands without a known IP among the last 10,000 seqs.
pub const UNKNOWN_IP_COMMANDS: &str =
    "SELECT COUNT(*) FROM commands WHERE seq > ? AND ip = 'unknown'";
/// Retention and purge, bounded by the rowid: `?` batch size, `?` cutoff.
pub const COMMANDS_DELETE_UPTO: &str = "DELETE FROM commands WHERE seq IN \
(SELECT seq FROM commands ORDER BY seq LIMIT ?) AND at <= ? \
RETURNING COALESCE(length(CAST(command AS BLOB)), 0)";
/// Export page: `?` after seq, `?` limit.
pub const EXPORT_COMMANDS: &str = "SELECT seq, at, command, command_len, mode, ip FROM commands \
WHERE seq > ? ORDER BY seq LIMIT ?";

// ---------------------------------------------------------------- VPS import

pub const IMPORTED_SEQ: &str = "SELECT seq FROM imported WHERE id = ?";
pub const IMPORTED_UPSERT: &str = "INSERT INTO imported (id, seq) VALUES (?, ?) \
ON CONFLICT(id) DO UPDATE SET seq = excluded.seq";
pub const IMPORTED_COUNT: &str = "SELECT COUNT(*) FROM imported";
