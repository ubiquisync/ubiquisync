//! SQL dialect: the points where SQL flavors genuinely diverge.
//!
//! The sync engine is storage-agnostic: it builds SQL as strings and runs
//! them through a backend connection. The few places where SQL flavors
//! actually differ — placeholder syntax, scalar-max function, collation,
//! conflict clauses, and type names (via [`DbType::sql_type`]) — are captured
//! here as a closed [`SqlDialect`] enum.
//!
//! A dialect is a property of the *SQL flavor*, not the driver: rusqlite, D1,
//! and Durable Objects are three backends but all speak [`SqlDialect::Sqlite`].
//! Backend crates (`ubiquisync-sqlite`, `ubiquisync-postgres`) therefore don't
//! implement a dialect — they only report which one they are. Centralizing the
//! divergences here keeps every cross-flavor difference in one auditable place.

/// The SQL flavor a backend speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlDialect {
    Sqlite,
    Postgres,
}

impl SqlDialect {
    /// Renders a positional bind placeholder for parameter `n` (1-based):
    /// `?n` on SQLite, `$n` on Postgres.
    pub fn placeholder(&self, n: usize) -> String {
        match self {
            SqlDialect::Sqlite => format!("?{n}"),
            SqlDialect::Postgres => format!("${n}"),
        }
    }

    /// Scalar two-argument max function: `MAX` on SQLite, `GREATEST` on
    /// Postgres. Callers must keep the `COALESCE` wrapping around the
    /// arguments — SQLite's `MAX` returns NULL if *any* argument is NULL
    /// while Postgres's `GREATEST` ignores NULLs; the COALESCE is what makes
    /// both backends merge identically. Do not simplify it away.
    pub fn scalar_max(&self) -> &'static str {
        match self {
            SqlDialect::Sqlite => "MAX",
            SqlDialect::Postgres => "GREATEST",
        }
    }

    /// Collation suffix appended to text comparisons that must order
    /// bytewise (the LWW value-byte tiebreak, pull-sync cursor iteration).
    /// Empty on SQLite (TEXT already compares with BINARY collation);
    /// ` COLLATE "C"` on Postgres, whose default collation is locale-aware.
    pub fn text_collate(&self) -> &'static str {
        match self {
            SqlDialect::Sqlite => "",
            SqlDialect::Postgres => " COLLATE \"C\"",
        }
    }

    pub fn without_rowid(&self) -> &'static str {
        match self {
            SqlDialect::Sqlite => " WITHOUT ROWID",
            SqlDialect::Postgres => "",
        }
    }
}

pub struct PlaceholderGen {
    dialect: SqlDialect,
    next_idx: usize,
}

impl PlaceholderGen {
    pub fn new(dialect: SqlDialect) -> Self {
        Self {
            dialect,
            next_idx: 1,
        }
    }

    pub fn next(&mut self) -> String {
        let p = self.dialect.placeholder(self.next_idx);
        self.next_idx += 1;
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_syntax_per_dialect() {
        assert_eq!(SqlDialect::Sqlite.placeholder(3), "?3");
        assert_eq!(SqlDialect::Postgres.placeholder(3), "$3");
    }

    #[test]
    fn scalar_max_per_dialect() {
        assert_eq!(SqlDialect::Sqlite.scalar_max(), "MAX");
        assert_eq!(SqlDialect::Postgres.scalar_max(), "GREATEST");
    }

    #[test]
    fn text_collate_per_dialect() {
        // SQLite already compares bytewise; Postgres needs an explicit "C".
        assert_eq!(SqlDialect::Sqlite.text_collate(), "");
        assert_eq!(SqlDialect::Postgres.text_collate(), " COLLATE \"C\"");
    }

    #[test]
    fn conflict_ignore_per_dialect() {
        // SQLite: the verb carries the ignore, no trailing clause.
        assert_eq!(SqlDialect::Sqlite.insert_ignore_verb(), "INSERT OR IGNORE");
        assert_eq!(SqlDialect::Sqlite.conflict_ignore_clause("\"id\""), "");
        // Postgres: plain verb plus an explicit DO NOTHING clause.
        assert_eq!(SqlDialect::Postgres.insert_ignore_verb(), "INSERT");
        assert_eq!(
            SqlDialect::Postgres.conflict_ignore_clause("\"id\""),
            " ON CONFLICT (\"id\") DO NOTHING"
        );
    }
}
