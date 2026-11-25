//! `SqlStore`: the core's `Store` over the Durable Object's `SqlStorage`, running the shared
//! statements of `core::ledger::sql`. Every cursor is read to the end before returning.

use worker::{SqlStorage, SqlStorageValue};

use crate::core::ledger::store::{Rows, SqlValue, Store, StoreError};

pub struct SqlStore {
    sql: SqlStorage,
    /// Fault injection: writes allowed before the next one fails (local builds only).
    #[cfg(zqs_fault_injection)]
    fail_after: Option<u32>,
    #[cfg(zqs_fault_injection)]
    writes: u32,
}

impl SqlStore {
    pub fn new(sql: SqlStorage) -> SqlStore {
        SqlStore {
            sql,
            #[cfg(zqs_fault_injection)]
            fail_after: None,
            #[cfg(zqs_fault_injection)]
            writes: 0,
        }
    }

    /// Makes the write after the first `writes` writes fail (local fault injection).
    #[cfg(zqs_fault_injection)]
    pub fn fail_after(&mut self, writes: Option<u32>) {
        self.fail_after = writes;
        self.writes = 0;
    }

    #[cfg(zqs_fault_injection)]
    fn inject(&mut self, sql: &str) -> Result<(), StoreError> {
        let head = sql.trim_start();
        let write = ["INSERT", "UPDATE", "DELETE", "REPLACE"]
            .iter()
            .any(|keyword| {
                head.get(..keyword.len())
                    .is_some_and(|start| start.eq_ignore_ascii_case(keyword))
            });
        if !write {
            return Ok(());
        }
        if let Some(limit) = self.fail_after {
            if self.writes >= limit {
                self.fail_after = None;
                return Err(StoreError(format!("injected failure after {limit} writes")));
            }
        }
        self.writes = self.writes.saturating_add(1);
        Ok(())
    }
}

fn to_storage(value: &SqlValue) -> SqlStorageValue {
    match value {
        SqlValue::Null => SqlStorageValue::Null,
        SqlValue::Int(i) => SqlStorageValue::Integer(*i),
        SqlValue::Real(f) => SqlStorageValue::Float(*f),
        SqlValue::Text(t) => SqlStorageValue::String(t.clone()),
        SqlValue::Blob(b) => SqlStorageValue::Blob(b.clone()),
    }
}

fn from_storage(value: SqlStorageValue) -> SqlValue {
    match value {
        SqlStorageValue::Null => SqlValue::Null,
        SqlStorageValue::Boolean(b) => SqlValue::Int(i64::from(b)),
        SqlStorageValue::Integer(i) => SqlValue::Int(i),
        SqlStorageValue::Float(f) => SqlValue::Real(f),
        SqlStorageValue::String(s) => SqlValue::Text(s),
        SqlStorageValue::Blob(b) => SqlValue::Blob(b),
    }
}

impl Store for SqlStore {
    fn exec(&mut self, sql: &'static str, params: &[SqlValue]) -> Result<Rows, StoreError> {
        #[cfg(zqs_fault_injection)]
        self.inject(sql)?;
        let bindings: Vec<SqlStorageValue> = params.iter().map(to_storage).collect();
        let cursor = self
            .sql
            .exec(sql, bindings)
            .map_err(|error| StoreError(error.to_string()))?;
        let mut rows = Vec::new();
        for row in cursor.raw() {
            let row = row.map_err(|error| StoreError(error.to_string()))?;
            rows.push(row.into_iter().map(from_storage).collect());
        }
        Ok(rows)
    }

    fn database_size(&mut self) -> Result<i64, StoreError> {
        Ok(i64::try_from(self.sql.database_size()).unwrap_or(i64::MAX))
    }
}

/// Creates the tables and the index (`CREATE … IF NOT EXISTS`, idempotent).
pub fn create_schema(sql: &SqlStorage) -> Result<(), StoreError> {
    let mut store = SqlStore::new(sql.clone());
    for statement in crate::core::ledger::sql::SCHEMA {
        store.exec(statement, &[])?;
    }
    Ok(())
}
