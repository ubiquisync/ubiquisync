use crate::{def_state, state::lww::Lww};

def_state!(NumberFormat {
    number_format: Lww<NumberFormatType>,
    decimal_places: Lww<DecimalPlaces>,
    no_decimal_grouping: Lww<bool>, // defaults to false, i.e. enabled
    currency_symbol: Lww<String>,        // defaults to "", which can maybe mean local currency?
     // TODO date time format options
});

#[derive(Default, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub enum DecimalPlaces {
    #[default]
    Auto,
    Fixed(u8),
}

#[derive(Default, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub enum NumberFormatType {
    #[default]
    General,
    Text,
    Number,
    Scientific,
    Percentage,
    Currency,
    DateTime,
}
