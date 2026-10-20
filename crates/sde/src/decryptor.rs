//! The decryptors an invention job can use: their chance, ME, TE and run changes.

use profit_core::industry::TypeId;
use profit_core::invention::Decryptor;
use rusqlite::{OptionalExtension, Row, params};

use crate::error::SdeError;
use crate::query::Sde;

impl Sde {
    /// The decryptors, by type id.
    pub fn decryptors(&self) -> Result<Vec<Decryptor>, SdeError> {
        let mut statement = self.conn.prepare(
            "SELECT type_id, chance_hundredths, me, te, runs FROM decryptors ORDER BY type_id",
        )?;
        let rows = statement
            .query_map([], decryptor_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The decryptor of this type, or `None` when the type is not one.
    pub fn decryptor(&self, type_id: TypeId) -> Result<Option<Decryptor>, SdeError> {
        let decryptor = self
            .conn
            .query_row(
                "SELECT type_id, chance_hundredths, me, te, runs FROM decryptors
                 WHERE type_id = ?1",
                params![type_id],
                decryptor_from_row,
            )
            .optional()?;
        Ok(decryptor)
    }
}

fn decryptor_from_row(row: &Row<'_>) -> rusqlite::Result<Decryptor> {
    Ok(Decryptor {
        type_id: row.get(0)?,
        chance_hundredths: row.get(1)?,
        me: row.get(2)?,
        te: row.get(3)?,
        runs: row.get(4)?,
    })
}
