//! Split SQL at SQLite statement boundaries without preparing it against a
//! local schema. The server remains responsible for syntax/name validation.
use crate::db::DbResult;

fn complete(sql: &str) -> DbResult<bool> {
    let sql = std::ffi::CString::new(sql).map_err(|_| "SQL contains a NUL byte")?;
    // SAFETY: CString supplies the terminated UTF-8 buffer SQLite expects.
    match unsafe { rusqlite::ffi::sqlite3_complete(sql.as_ptr()) } {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err("SQLite could not check statement completeness".into()),
    }
}

fn without_trivia(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start_matches(|c: char| c.is_whitespace() || c == ';' || c == '\u{feff}');
        if let Some(rest) = sql.strip_prefix("--") {
            sql = rest.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(rest) = sql.strip_prefix("/*") {
            let Some((_, tail)) = rest.split_once("*/") else {
                return "";
            };
            sql = tail;
        } else {
            return sql;
        }
    }
}

pub fn changes_transaction(sql: &str) -> bool {
    let keyword = without_trivia(sql)
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("");
    matches!(
        keyword.to_ascii_uppercase().as_str(),
        "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE"
    )
}

fn has_sql(sql: &str) -> bool {
    !without_trivia(sql).is_empty()
}

/// The server parses this as one SELECT. Preserve the user's own LIMIT and
/// put the closing parenthesis on a new line after any trailing comment.
pub fn select_source(sql: &str) -> DbResult<String> {
    let parts = split(sql)?;
    if parts.len() != 1 {
        return Err("output source must contain exactly one SELECT".into());
    }
    Ok(format!(
        "SELECT * FROM (\n{}\n)",
        parts[0].trim_end_matches(';')
    ))
}

pub fn split(sql: &str) -> DbResult<Vec<&str>> {
    if sql.contains('\0') {
        return Err("SQL contains a NUL byte".into());
    }
    let b = sql.as_bytes();
    let (mut start, mut i) = (0, 0);
    let mut statements = Vec::new();
    while i < b.len() {
        match b[i] {
            b'\'' | b'"' | b'`' | b'[' => {
                let end = if b[i] == b'[' { b']' } else { b[i] };
                i += 1;
                while i < b.len() {
                    if b[i] == end {
                        i += 1;
                        if end != b']' && b.get(i) == Some(&end) {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i - 1] == b'*' && b[i] == b'/') {
                    i += 1;
                }
                i = (i + 1).min(b.len());
            }
            b';' => {
                i += 1;
                // sqlite3_complete recognizes CREATE TRIGGER ... END;
                // including CASE ... END expressions inside the body.
                if complete(&sql[start..i])? {
                    if has_sql(&sql[start..i]) {
                        statements.push(sql[start..i].trim());
                    }
                    start = i;
                }
            }
            _ => i += 1,
        }
    }
    let tail = &sql[start..];
    if has_sql(tail) {
        if !complete(&format!("{tail}\n;"))? {
            return Err("incomplete SQL statement".into());
        }
        statements.push(tail.trim());
    }
    Ok(statements)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sqlite_boundaries_preserve_literals_comments_and_triggers() {
        let sql = "-- leading ;\nCREATE TABLE [t; x](`a;b` TEXT); /* ; */
            INSERT INTO [t; x] VALUES ('Ada; O''Brien; 雨');
            CREATE TRIGGER \"tr;g\" AFTER INSERT ON [t; x] BEGIN
              UPDATE [t; x] SET `a;b`=CASE WHEN 1 THEN 'end;' ELSE 'no' END;
              INSERT INTO [t; x] VALUES ('again;');
            END; SELECT \"a;b\" FROM [t; x] -- trailing ;";
        let parts = split(sql).unwrap();
        assert_eq!(parts.len(), 4);
        let db = rusqlite::Connection::open_in_memory().unwrap();
        for part in &parts[..3] {
            db.execute_batch(part).unwrap();
        }
        assert_eq!(
            db.query_row(parts[3], [], |r| r.get::<_, String>(0))
                .unwrap(),
            "Ada; O'Brien; 雨"
        );
        assert!(split(" ; -- no SQL;\n /* empty; */").unwrap().is_empty());
        assert!(split("SELECT 'unclosed").is_err());
        assert!(split("CREATE TRIGGER t AFTER INSERT ON x BEGIN SELECT 1;").is_err());
        assert!(split("SELECT 1;\0DELETE FROM x").is_err());
        assert!(changes_transaction(
            " -- leading ;\n /* comment */ BEGIN IMMEDIATE"
        ));
        assert!(changes_transaction("; SAVEPOINT inner"));
        assert!(changes_transaction("\u{feff}BEGIN"));
        assert!(!changes_transaction("SELECT 'BEGIN; COMMIT;'"));
    }
}
