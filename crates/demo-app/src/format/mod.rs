use borsh::{BorshDeserialize, BorshSerialize};

use crate::{def_state, state::lww::Lww};

def_state!(NumberFormat {
    number_format: Lww<NumberFormatType>,
    decimal_places: Lww<DecimalPlaces>,
    no_decimal_grouping: Lww<bool>, // defaults to false, i.e. enabled
    currency_symbol: Lww<String>,        // defaults to "", which can maybe mean local currency?
     // TODO date time format options
});

#[derive(
    Debug, Default, Clone, PartialEq, PartialOrd, Eq, Ord, BorshSerialize, BorshDeserialize,
)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum DecimalPlaces {
    #[default]
    Auto = 0,
    Fixed(u8) = 1,
}

#[derive(
    Debug, Default, Clone, PartialEq, PartialOrd, Eq, Ord, BorshSerialize, BorshDeserialize,
)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum NumberFormatType {
    #[default]
    General = 0,
    Text = 1,
    Number = 2,
    Scientific = 3,
    Percentage = 4,
    Currency = 5,
    DateTime = 6,
}
