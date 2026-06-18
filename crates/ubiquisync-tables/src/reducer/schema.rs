use crate::db::Db;
use crate::id::{ColumnId, TableId};
use crate::reducer::ReducerError;
use crate::reducer::surrogate::{surrogate_col_name, surrogate_pk_name, surrogate_table_name};
use crate::reducer::util::{lww_col_name, quote_ident};
use std::collections::HashMap;

