use crate::pricing::quote as item_quote;
use crate::shipping;

pub fn total(unit_cents: u64, quantity: u32, discount_bps: u16, remote: bool) -> u64 {
    item_quote(unit_cents, quantity, discount_bps) + shipping::quote(remote)
}
