//! Native test store: the same SQL on real SQLite (rusqlite, bundled), inside `BEGIN … COMMIT`,
//! with a switch that fails the next commit to test rollback and cache invalidation.

use std::cell::RefCell;

use rusqlite::types::{Value as RValue, ValueRef};
use rusqlite::Connection;

use super::serve::LedgerHost;
use super::sql::SCHEMA;
use super::store::{Rows, SqlValue, Store, StoreError};
use super::{Book, LedgerError};

pub struct SqliteStore {
    pub conn: Connection,
    /// Makes the next commit fail (and roll back).
    pub fail_next_commit: bool,
    /// Makes the next statement whose text contains this marker fail.
    pub fail_statement: Option<&'static str>,
    /// Statements run, for assertions on access patterns.
    pub executed: Vec<&'static str>,
}

impl SqliteStore {
    pub fn open() -> SqliteStore {
        let conn = Connection::open_in_memory().unwrap();
        for statement in SCHEMA {
            conn.execute_batch(statement).unwrap();
        }
        SqliteStore {
            conn,
            fail_next_commit: false,
            fail_statement: None,
            executed: Vec::new(),
        }
    }

    /// `EXPLAIN QUERY PLAN` details of a statement.
    pub fn plan(&self, sql: &str, params: usize) -> Vec<String> {
        let mut stmt = self
            .conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let values: Vec<RValue> = (0..params).map(|_| RValue::Integer(1)).collect();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(3)
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    /// Rows of an ad-hoc query, for assertions.
    pub fn rows(&mut self, sql: &str) -> Rows {
        let mut stmt = self.conn.prepare(sql).unwrap();
        let count = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            out.push(
                (0..count)
                    .map(|i| convert(row.get_ref(i).unwrap()))
                    .collect(),
            );
        }
        out
    }

    pub fn count(&mut self, sql: &str) -> i64 {
        self.rows(sql)[0][0].as_i64().unwrap()
    }
}

fn convert(value: ValueRef<'_>) -> SqlValue {
    match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(i) => SqlValue::Int(i),
        ValueRef::Real(f) => SqlValue::Real(f),
        ValueRef::Text(t) => SqlValue::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => SqlValue::Blob(b.to_vec()),
    }
}

fn param(value: &SqlValue) -> RValue {
    match value {
        SqlValue::Null => RValue::Null,
        SqlValue::Int(i) => RValue::Integer(*i),
        SqlValue::Real(f) => RValue::Real(*f),
        SqlValue::Text(t) => RValue::Text(t.clone()),
        SqlValue::Blob(b) => RValue::Blob(b.clone()),
    }
}

impl Store for SqliteStore {
    fn exec(&mut self, sql: &'static str, params: &[SqlValue]) -> Result<Rows, StoreError> {
        self.executed.push(sql);
        if let Some(marker) = self.fail_statement {
            if sql.contains(marker) {
                self.fail_statement = None;
                return Err(StoreError(format!("injected failure: {marker}")));
            }
        }
        let mut stmt = self
            .conn
            .prepare_cached(sql)
            .map_err(|e| StoreError(e.to_string()))?;
        let count = stmt.column_count();
        let values: Vec<RValue> = params.iter().map(param).collect();
        let mut rows = stmt
            .query(rusqlite::params_from_iter(values))
            .map_err(|e| StoreError(e.to_string()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| StoreError(e.to_string()))? {
            let mut values = Vec::with_capacity(count);
            for i in 0..count {
                values.push(convert(
                    row.get_ref(i).map_err(|e| StoreError(e.to_string()))?,
                ));
            }
            out.push(values);
        }
        Ok(out)
    }

    fn database_size(&mut self) -> Result<i64, StoreError> {
        let pages: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .map_err(|e| StoreError(e.to_string()))?;
        let size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .map_err(|e| StoreError(e.to_string()))?;
        Ok(pages * size)
    }
}

/// `LedgerHost` over one in-memory database: the cached book and the store, as the Durable
/// Object keeps them between requests.
pub struct SqliteHost {
    pub store: RefCell<SqliteStore>,
    pub book: RefCell<Option<Book>>,
    /// Transactions committed and rolled back.
    pub commits: RefCell<usize>,
    pub rollbacks: RefCell<usize>,
}

impl Default for SqliteHost {
    fn default() -> SqliteHost {
        SqliteHost::new()
    }
}

impl SqliteHost {
    pub fn new() -> SqliteHost {
        SqliteHost {
            store: RefCell::new(SqliteStore::open()),
            book: RefCell::new(None),
            commits: RefCell::new(0),
            rollbacks: RefCell::new(0),
        }
    }

    /// A Durable Object eviction: the in-memory book is gone, storage stays.
    pub fn evict(&self) {
        *self.book.borrow_mut() = None;
    }

    pub fn cached(&self) -> bool {
        self.book.borrow().is_some()
    }
}

impl LedgerHost for SqliteHost {
    type Store = SqliteStore;

    fn transact<T: 'static>(
        &self,
        now: i64,
        work: impl FnOnce(&mut Book, &mut SqliteStore) -> Result<T, LedgerError> + 'static,
    ) -> Result<T, LedgerError> {
        let mut store = self.store.borrow_mut();
        let mut cell = self.book.borrow_mut();
        store.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let loaded = match cell.take() {
            Some(book) => Ok(book),
            None => Book::load(&mut *store, now),
        };
        let result = match loaded {
            Ok(book) => {
                let book = cell.insert(book);
                work(book, &mut store)
            }
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(_) if store.fail_next_commit => {
                store.fail_next_commit = false;
                Err(LedgerError::Store("commit failed".into()))
            }
            other => other,
        };
        match result {
            Ok(value) => {
                store.conn.execute_batch("COMMIT").unwrap();
                *self.commits.borrow_mut() += 1;
                Ok(value)
            }
            Err(error) => {
                store.conn.execute_batch("ROLLBACK").unwrap();
                *self.rollbacks.borrow_mut() += 1;
                *cell = None;
                Err(error)
            }
        }
    }
}
