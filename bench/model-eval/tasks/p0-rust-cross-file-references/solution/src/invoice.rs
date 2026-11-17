use crate::pricing::quote_cents;

pub fn line_total(unit_cents: u64, quantity: u32, discount_bps: u16) -> u64 {
    quote_cents(unit_cents, quantity, discount_bps)
}
