//! Cooperative cancellation and progress for complete, read-only output jobs.
use crate::db::{DbLink, DbResult, QueryEvent, QueryResult};
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Clone, Default)]
pub struct Control(Arc<State>);

#[derive(Default)]
struct State {
    // 0 working, 1 cancelled, 2 publishing. Cancellation cannot undo a
    // publication that has already begun.
    phase: AtomicU8,
    rows: AtomicUsize,
}

impl Control {
    pub fn cancel(&self) -> bool {
        self.0
            .phase
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            || self.cancelled()
    }
    pub fn cancelled(&self) -> bool {
        self.0.phase.load(Ordering::SeqCst) == 1
    }
    pub fn check(&self) -> DbResult<()> {
        if self.cancelled() {
            Err("operation cancelled".into())
        } else {
            Ok(())
        }
    }
    pub fn row(&self) -> DbResult<()> {
        self.check()?;
        self.0.rows.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    pub fn rows(&self) -> usize {
        self.0.rows.load(Ordering::Relaxed)
    }
    pub fn publish(&self) -> DbResult<()> {
        self.0
            .phase
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| "operation cancelled".into())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Pager {
        title: String,
        lines: Vec<String>,
        file_stem: String,
    },
    Message(String),
}

pub fn collect(db: &dyn DbLink, sql: &str, control: &Control) -> DbResult<QueryResult> {
    control.check()?;
    let started = std::time::Instant::now();
    let data = Arc::new(Mutex::new((Vec::new(), Vec::new())));
    let output = data.clone();
    let progress = control.clone();
    db.stream_query(
        sql,
        Box::new(move |event| {
            progress.check()?;
            let mut data = output.lock().unwrap();
            match event {
                QueryEvent::Columns(columns) => data.0 = columns,
                QueryEvent::Row(row) => {
                    progress.row()?;
                    data.1.push(row);
                }
                QueryEvent::End => (),
            }
            Ok(())
        }),
    )?;
    let (columns, rows) = std::mem::take(&mut *data.lock().unwrap());
    Ok(QueryResult {
        columns,
        rows,
        truncated: false,
        elapsed: started.elapsed(),
    })
}
