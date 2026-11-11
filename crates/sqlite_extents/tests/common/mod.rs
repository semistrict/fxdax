// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A database a fork of its volume and the volume change apart: what each test of forks does, on
//! each kind of device.

use rusqlite::{Connection, params};
use std::collections::BTreeMap;

/// The rows a database's table holds, by key.
pub type Rows = BTreeMap<i64, Vec<u8>>;

const ROWS: i64 = 3000;
const LEN: usize = 400;

/// Fills the table `t` of the database on `connection`: rows in several transactions, then some
/// updated a row a transaction, so that what they replaced is still to be freed as it forks.
pub fn fill(connection: &Connection) -> Rows {
    connection.execute_batch("CREATE TABLE t (k INTEGER PRIMARY KEY, v BLOB)").unwrap();
    let mut rows = Rows::new();
    for batch in 0..6 {
        let transaction = connection.unchecked_transaction().unwrap();
        for k in batch * ROWS / 6..(batch + 1) * ROWS / 6 {
            let v = vec![k as u8; LEN];
            transaction.execute("INSERT INTO t VALUES (?1, ?2)", params![k, v]).unwrap();
            rows.insert(k, v);
        }
        transaction.commit().unwrap();
    }
    for k in (0..ROWS).step_by(101) {
        set(connection, &mut rows, k, b'u');
    }
    rows
}

fn set(connection: &Connection, rows: &mut Rows, k: i64, byte: u8) {
    let v = vec![byte; LEN];
    connection.execute("INSERT OR REPLACE INTO t VALUES (?1, ?2)", params![k, v]).unwrap();
    rows.insert(k, v);
}

/// The parent's changes after the fork: half its rows in a transaction, a few more one at a time.
/// The other half it keeps where the fork found them.
pub fn change_parent(connection: &Connection, rows: &mut Rows) {
    let transaction = connection.unchecked_transaction().unwrap();
    for k in 0..ROWS / 2 {
        set(&transaction, rows, k, b'p');
    }
    transaction.commit().unwrap();
    for k in (0..ROWS / 2).step_by(97) {
        set(connection, rows, k, b'P');
    }
}

/// The fork's changes: every row it inherited rewritten, so that it frees all it inherited; some
/// deleted, some added.
pub fn change_fork(connection: &Connection, rows: &mut Rows) {
    let transaction = connection.unchecked_transaction().unwrap();
    for k in 0..ROWS {
        set(&transaction, rows, k, b'f');
    }
    transaction.commit().unwrap();
    connection.execute("DELETE FROM t WHERE k >= ?1", params![ROWS - 500]).unwrap();
    rows.retain(|k, _| *k < ROWS - 500);
    for k in ROWS..ROWS + 100 {
        set(connection, rows, k, b'F');
    }
    connection.execute_batch("VACUUM").unwrap();
}

/// Checks the database on `connection` is whole and holds `expected`.
pub fn check(connection: &Connection, expected: &Rows) {
    let integrity: String =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
    assert_eq!(integrity, "ok");
    let mut statement = connection.prepare("SELECT k, v FROM t ORDER BY k").unwrap();
    let rows: Rows = (statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap())
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), expected.len());
    assert!(rows == *expected, "the rows differ from those expected");
}
