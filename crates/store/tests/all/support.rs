use std::path::Path;

use rusqlite::Connection;

/// Undoes migration 6 (the costs), so the file reads as a version-5 one again.
pub(crate) fn undo_version_six(path: &Path) {
    let conn = Connection::open(path).expect("open");
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE fee_settings;
         ALTER TABLE facilities DROP COLUMN tax_ppm;
         ALTER TABLE facilities DROP COLUMN system;
         ALTER TABLE plans DROP COLUMN sell_mode;
         DELETE FROM migrations WHERE version = 6;
         PRAGMA user_version = 5;",
    )
    .expect("downgrade");
}
