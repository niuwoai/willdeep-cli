use crate::pricing::quote;

pub fn line_total(unit_cents: u64, quantity: u32, discount_bps: u16) -> u64 {
    quote(unit_cents, quantity, discount_bps)
}
