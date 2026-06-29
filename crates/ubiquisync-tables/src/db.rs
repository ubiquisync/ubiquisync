impl DbType {
    pub fn is_valid_for(self, col_type: ColType) -> bool {
        match col_type {
            ColType::Bytes => self == DbType::Blob,
            ColType::Text => self == DbType::Text,
            ColType::I64 => self == DbType::Integer,
            ColType::Uuid => self == DbType::Uuid || self == DbType::Blob,
        }
    }
}
