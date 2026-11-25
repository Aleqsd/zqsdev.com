//! Storage seen by the ledger: run one SQL statement (from `sql`) with positional parameters and
//! get its rows back. The Durable Object implements it over `SqlStorage` inside `transactionSync`;
//! native tests over rusqlite inside `BEGIN … COMMIT`. Typed reads are built on top here, so both
//! stores share them.

use std::fmt;

/// One SQL value (the SQLite storage classes).
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    /// An integer; whole floats are accepted (JavaScript returns numbers).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            SqlValue::Int(i) => Some(*i),
            SqlValue::Real(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            SqlValue::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn into_text(self) -> Option<String> {
        match self {
            SqlValue::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<i64> for SqlValue {
    fn from(v: i64) -> SqlValue {
        SqlValue::Int(v)
    }
}

impl From<&str> for SqlValue {
    fn from(v: &str) -> SqlValue {
        SqlValue::Text(v.to_string())
    }
}

impl From<String> for SqlValue {
    fn from(v: String) -> SqlValue {
        SqlValue::Text(v)
    }
}

impl<T: Into<SqlValue>> From<Option<T>> for SqlValue {
    fn from(v: Option<T>) -> SqlValue {
        v.map_or(SqlValue::Null, Into::into)
    }
}

/// A failed statement (or commit): the transaction is rolled back and the cached book dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Rows = Vec<Vec<SqlValue>>;

pub trait Store {
    /// Runs one statement (all its rows are read before returning).
    fn exec(&mut self, sql: &'static str, params: &[SqlValue]) -> Result<Rows, StoreError>;
    /// Bytes of the database file (`ctx.storage.sql.databaseSize`).
    fn database_size(&mut self) -> Result<i64, StoreError>;
}

/// The first column of the first row as an integer (`None` for no row or NULL).
pub fn query_i64(
    store: &mut impl Store,
    sql: &'static str,
    params: &[SqlValue],
) -> Result<Option<i64>, StoreError> {
    let rows = store.exec(sql, params)?;
    Ok(rows
        .first()
        .and_then(|row| row.first())
        .and_then(SqlValue::as_i64))
}

/// An aggregate that always returns one integer (`COUNT`, `COALESCE(SUM…)`).
pub fn query_count(
    store: &mut impl Store,
    sql: &'static str,
    params: &[SqlValue],
) -> Result<i64, StoreError> {
    query_i64(store, sql, params)?
        .ok_or_else(|| StoreError(format!("no integer result for: {sql}")))
}

/// Column `i` of a row as an integer.
pub fn col_i64(row: &[SqlValue], i: usize) -> Option<i64> {
    row.get(i).and_then(SqlValue::as_i64)
}

/// Column `i` of a row as text.
pub fn col_text(row: &[SqlValue], i: usize) -> Option<String> {
    row.get(i).and_then(|v| v.as_str().map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_conversions() {
        assert_eq!(SqlValue::Int(7).as_i64(), Some(7));
        assert_eq!(SqlValue::Real(7.0).as_i64(), Some(7));
        assert_eq!(SqlValue::Real(7.5).as_i64(), None);
        assert_eq!(SqlValue::Real(f64::NAN).as_i64(), None);
        assert_eq!(SqlValue::Text("7".into()).as_i64(), None);
        assert_eq!(SqlValue::from(None::<String>), SqlValue::Null);
        assert_eq!(SqlValue::from(Some("a")), SqlValue::Text("a".into()));
        assert_eq!(col_text(&[SqlValue::Null], 0), None);
        assert_eq!(col_i64(&[], 3), None);
    }
}
